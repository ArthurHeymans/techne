;;; Indentation, as Emacs's: TAB indents the line (with the region active,
;;; its lines) as the buffer's mode does, RET indents the line it opens,
;;; C-M-\ indents the region, M-m goes to the line's indentation; the modal
;;; profile's = operator, o and O do the same. A mode indents with its
;;; #:indent procedure, (document starts) -> edits, each giving a line's
;;; leading blanks, or the start of them, blanks; Scheme's is Lisp's
;;; (techne-editor's lisp_indent), `set-indent!` saying how a form
;;; indents, as Emacs's `scheme-indent-function` property.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")

(provide indentation line-starts indent-lines! indent-here! newline-indent!
         lisp-indent-specs set-indent! lisp-indent
         indent-for-tab-command newline-and-indent indent-region back-to-indentation)

(define not-blank (string->regexp "[^ \t]"))

;; The first position from FROM, in its line's leading blanks, that is
;; not a blank: one search over the rope, not a call a character, as a
;; line indented under a bracket far out on a long line has a megabyte of
;; blanks.
(define (blanks-end d from)
  (let ((end (line-end d from))
        (m (search-text-regexp d from not-blank #t)))
    (if m (min (car m) end) end)))

(define (indentation d p)
  "Return the position of the first character not a blank on P's line.
P is a position of document D; the line's end if it is all blanks."
  (blanks-end d (line-start d p)))

(define (line-starts d from to)
  "Return the starts of the lines of document D from FROM to TO, in order.
They are FROM's line and those starting before TO."
  (let loop ((p (line-start d from)) (acc '()))
    (let ((next (cadr (line-span d p))))
      (if (or (>= next to) (>= next (document-length d)) (= next p))
          (reverse (cons p acc))
          (loop next (cons p acc))))))

(define (indent-lines! s starts group)
  "Indent the lines of S's document at STARTS as its buffer's mode does.
STARTS are sorted, or a procedure of no arguments giving them, called
only if the mode indents; the edits are one change, GROUP as `edit!`
takes it. Return the edits made, each (from to text) in the text before
them, or #f if the mode does not indent."
  (let ((indent (let ((b (current-buffer s))) (and b (buffer-indent b)))))
    (and indent
         (let ((edits (indent (doc s) (if (procedure? starts) (starts) starts))))
           (unless (null? edits) (edit! s edits group))
           edits))))

;; The ends of the blanks EDITS made, keyed by their lines' starts after
;; them: EDITS are sorted, each replacing a line's leading blanks, or the
;; start of them, with blanks, whose characters are bytes (`string-length`
;; is constant time on ASCII; `string->utf8` would copy a megabyte of
;; them). More blanks may follow those an edit made.
(define (indented-lines edits)
  (let ((ends (make-hash-table)))
    (let loop ((edits edits) (shift 0))
      (unless (null? edits)
        (let* ((e (car edits))
               (from (+ (car e) shift))
               (n (string-length (caddr e))))
          (hash-table-set! ends from (+ from n))
          (loop (cdr edits) (+ shift n (- (car e) (cadr e)))))))
    ends))

;; Heads in their line's leading blanks go to its indentation; on the
;; lines EDITS indented, the blanks they made are not scanned again, as
;; they may be a megabyte.
(define (to-indentation! s edits)
  (let ((d (doc s)) (ends (indented-lines edits)))
    (move! s (lambda (h)
               (let* ((start (line-start d h))
                      (i (blanks-end d (hash-table-ref/default ends start start))))
                 (if (<= h i) i h))))))

;; The starts of the lines the ranges of S are on, sorted, each once.
(define (range-lines s whole)
  (let ((d (doc s)))
    (let loop ((starts
                (sort (append-map (lambda (r)
                                    (if whole
                                        (line-starts d (min (car r) (cadr r)) (max (car r) (cadr r)))
                                        (list (line-start d (cadr r)))))
                                  (ranges s))
                      <))
               (unique '()))
      ;; Equal offsets are adjacent after sorting. Scanning all previous
      ;; offsets for every line would make a large selection quadratic.
      (cond ((null? starts) (reverse unique))
            ((and (pair? unique) (= (car starts) (car unique)))
             (loop (cdr starts) unique))
            (else (loop (cdr starts) (cons (car starts) unique)))))))

(define (indent-here! s group)
  "Indent the lines of the heads of S's ranges, as the buffer's mode does.
Heads in a line's leading blanks go past them. GROUP is as `edit!`
takes it. Return #f if the mode does not indent."
  ;; The lines found only where the mode indents: a line's start is far
  ;; on a long one.
  (let ((edits (indent-lines! s (lambda () (range-lines s #f)) group)))
    (and edits (begin (to-indentation! s edits) #t))))

(define (newline-indent! s group)
  "Break the line at every range of S, indenting the lines it opens.
It is one change, GROUP as `edit!` takes it."
  (insert-text! s "\n" group)
  (indent-here! s 'extend))

(define-command (indent-for-tab-command s n)
  "Indent the line as the buffer's mode does; with the region, its lines.
Point in the line's leading blanks goes to its indentation. Where the
mode does not indent, insert a tab."
  (cond ((sget s 'extend) (unless (indent-lines! s (lambda () (range-lines s #t)) 'new) (insert-text! s "\t" 'new)))
        ((not (indent-here! s 'new)) (insert-text! s "\t" 'new))))

(define-command (newline-and-indent s n)
  "Break the line and indent the new one, as the buffer's mode does."
  (newline-indent! s 'new))

(define-command (indent-region s n)
  "Indent the lines of the region as the buffer's mode does."
  (unless (indent-lines! s (lambda () (range-lines s #t)) 'new)
    (message! s "This buffer's mode does not indent"))
  (sset! s 'extend #f))

(define-command (back-to-indentation s n)
  "Go to the first character of the line that is not a blank."
  (let ((d (doc s))) (move! s (lambda (h) (indentation d h)))))

;;; Lisp

(define lisp-indent-specs (make-indent-specs)
  "How Lisp forms indent: see `set-indent!`.")

(define (set-indent! name n)
  "Make the form NAME indent its first N arguments by 4, its body by 2.
NAME is a symbol, as Emacs's `scheme-indent-function` property is put;
with N #f it indents as a call. Names starting with \"def\" have a body."
  (indent-spec-set! lisp-indent-specs (symbol->string name) n))

(define (lisp-indent d starts)
  "Return the edits (from to text) indenting the lines at STARTS as Lisp.
STARTS are sorted line starts of document D."
  (lisp-indent-edits d starts lisp-indent-specs))
