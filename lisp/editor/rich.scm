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
;;;   (break)                        a line break inside a paragraph
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
(define-face rich.code-label "The language a code block is in." #:fg 'overlay #:slant 'italic)
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
           ((eq? (car x) 'span)
            (if (< (caddr x) (cadddr x)) (list (vector "" face (cadr x) (caddr x) (cadddr x) #f #f)) '()))
           ((eq? (car x) 'break) (list (gen " " face)))
           ((memq (car x) '(strong emph strike code)) (inline-runs (cdr x) (styled face (car x))))
           ((eq? (car x) 'link)
            (map (lambda (r) (with-target r (cadr x))) (inline-runs (cddr x) (styled face 'link))))
           ((eq? (car x) 'image)
            (map (lambda (r) (with-picture (with-target r (cadr x)) (picture-of (cadr x))))
                 (cons (gen "▣ " 'rich.image) (inline-runs (cddr x) (if (whole? face) face 'rich.image)))))
           (else '())))
   xs))

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
    ((paragraph) (list (cons* 'text first (inline-runs (cdr b) face))))
    ((heading)
     (let* ((level (cadr b)) (hf (heading-face level)) (runs (inline-runs (cddr b) hf)))
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
;; that changed; the table is emptied when it grows past a few hundred.
(define %code-faces (make-hash-table))
(define (code-faces language lines)
  (let ((lang (rich-code-language language)))
    (if (not lang)
        '()
        (let* ((text (string-join (map line-text lines) "\n"))
               (key (string-append (symbol->string lang) "\x0;" text)))
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
      (cond ((= i len) (reverse (if (< from i) (cons (piece from i (vector-ref paint from)) acc) acc)))
            ((eq? (vector-ref paint i) (vector-ref paint from)) (loop (+ i 1) from acc))
            (else (loop (+ i 1) i (cons (piece from i (vector-ref paint from)) acc)))))))

(define (utf8-substring s from to)
  (utf8->string (bytevector-copy (string->utf8 s) from to)))

(define (code-rows language lines first rest)
  (let* ((faces (code-faces language lines))
         (pad (list (gen " " #f)))
         (label (if language (list (cons* 'rich.code-block first (list (gen (string-append " " (if (symbol? language) (symbol->string language) language)) 'rich.code-label)))) '()))
         (lines (if (null? lines) (list "") lines)))
    (let loop ((ls lines) (offset 0) (prefix (if (null? label) first rest)) (acc (reverse label)))
      (if (null? ls)
          (reverse acc)
          (loop (cdr ls) (+ offset (utf8-length (line-text (car ls))) 1) rest
                (cons (cons* 'rich.code-block prefix (append pad (painted-runs (car ls) offset faces))) acc))))))

;;; Tables, drawn with lines, in the fixed-pitch font so they line up.

(define (table-rows aligns header body first rest)
  (let* ((cells (lambda (row f) (map (lambda (c) (inline-runs c f)) row)))
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

;;; Buffers of rich text. Their state is a table, plain data: the links
;;; of each row, (from to target) in bytes of the row; the rows with a
;;; face of their own, (key bytes-of-prefix face); the rows that hang
;;; under their prefix, (key . bytes-of-prefix); the pictures shown above
;;; rows, (key . path). What the buffer's maker keeps beside is its own.

(define (run->spec r)
  (if (vector-ref r 2)
      (list (vector-ref r 2) (vector-ref r 3) (vector-ref r 4) (vector-ref r 1))
      (list (run-text r) (vector-ref r 1))))

(define (rich-present! b blocks)
  "Show the rich text BLOCKS in buffer B, a buffer of `make-rich-buffer!'.
Only rows that differ change: a caret on a row that stays, stays.
Pictures not loaded yet load in the background and show when they are."
  (let ((st (buffer-state b)) (links (make-hash-table)))
    (let loop ((rs (render-blocks blocks '() '() 'rich.text 0 #t)) (i 0) (out '()) (faces '()) (hangs '()) (pictures '()))
      (if (null? rs)
          (begin
            (present! (buffer-document b) (reverse out))
            (hash-table-set! st 'links links)
            (hash-table-set! st 'row-faces faces)
            (hash-table-set! st 'hangs hangs)
            (hash-table-set! st 'pictures pictures)
            (load-pictures! (map cdr pictures)))
          (let* ((r (car rs)) (key (string-append "r" (number->string i))) (runs (append (cadr r) (cddr r)))
                 (prefix (fold (lambda (r n) (+ n (run-bytes r))) 0 (cadr r)))
                 (picture (any (lambda (r) (vector-ref r 6)) (cddr r))))
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
            (loop (cdr rs) (+ i 1)
                  (cons (row (map run->spec runs) #:key key) out)
                  (if (eq? (car r) 'text) faces (cons (list key prefix (car r)) faces))
                  (if (and (> prefix 0) (eq? (car r) 'text)) (cons (cons key prefix) hangs) hangs)
                  (if picture (cons (cons key picture) pictures) pictures)))))))

;; Load the pictures at PATHS not loaded yet, each in a task of its own:
;; once it is, the projections show it. One that fails stays its
;; description.
(define %loading (make-hash-table))
(define (load-pictures! paths)
  (for-each (lambda (path)
              (unless (or (image-loaded path) (hash-table-ref/default %loading path #f))
                (hash-table-set! %loading path #t)
                (spawn (lambda ()
                         (guard (e (#t #f)) (image-load path))
                         (hash-table-delete! %loading path)))))
            paths))

(define (make-rich-buffer! name #:mode [mode 'rich-mode] #:state [state (make-hash-table)])
  "Return a new buffer NAME for rich text, in MODE.
MODE is `rich-mode' or one extending it. STATE is a table the buffer
keeps; `rich-present!' adds its own entries."
  (make-generated-buffer! name (make-presentation) mode #:state state))

;; The rows with a face of their own have it after their prefix and
;; through their line break, so a background or an underline reaches the
;; pane's edge.
(define (row-faces p from to)
  (let* ((b (document-buffer p)) (st (and b (buffer-state b))))
    (if (not (hash-table? st))
        '()
        (filter-map (lambda (c)
                      (let ((span (presentation-row-span p (car c))))
                        (and span (<= (car span) to) (>= (cadr span) from)
                             (list (+ (car span) (cadr c)) (+ (cadr span) 1) (caddr c)))))
                    (hash-table-ref/default st 'row-faces '())))))

;; Rows under bullets and bars hang under their text; pictures loaded
;; show above their rows.
(define (rich-projections p from to)
  (let* ((b (document-buffer p)) (st (and b (buffer-state b))))
    (if (not (hash-table? st))
        '()
        (let ((within (lambda (key) (let ((span (presentation-row-span p key)))
                                      (and span (<= (car span) to) (>= (cadr span) from) span)))))
          (append
           (filter-map (lambda (h) (let ((span (within (car h)))) (and span (list 'hang (car span) (+ (car span) (cdr h))))))
                       (hash-table-ref/default st 'hangs '()))
           (filter-map (lambda (x) (let ((span (within (car x))) (image (image-loaded (cdr x))))
                                     (and span image (list 'block (car span) image))))
                       (hash-table-ref/default st 'pictures '())))))))

;; The link at POS, else the source of the text there.
(define (rich-target b pos)
  (let* ((st (buffer-state b)) (p (buffer-document b)) (key (presentation-key-at p pos)))
    (and (hash-table? st)
         (or (and key
                  (let* ((start (car (presentation-row-span p key))) (rel (- pos start))
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
