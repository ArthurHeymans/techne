;;; Indentation, as Emacs's: TAB indents the line (with the region active,
;;; its lines) as the buffer's mode does, RET indents the line it opens,
;;; C-M-\ indents the region, M-m goes to the line's indentation; the modal
;;; profile's = operator, o and O do the same. A mode indents with its
;;; #:indent procedure, (document starts) -> edits; Scheme's is Lisp's
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

(define (indentation d p)
  "Return the position of the first character not a blank on P's line.
P is a position of document D; the line's end if it is all blanks."
  (let ((end (line-end d p)))
    (let loop ((i (line-start d p)))
      (if (and (< i end) (member (document-substring d i (+ i 1)) '(" " "\t")))
          (loop (+ i 1))
          i))))

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
takes it. Return #f if the mode does not indent."
  (let ((indent (let ((b (current-buffer s))) (and b (buffer-indent b)))))
    (and indent
         (let ((edits (indent (doc s) (if (procedure? starts) (starts) starts))))
           (unless (null? edits) (edit! s edits group))
           #t))))

;; Heads in their line's leading blanks go to its indentation.
(define (to-indentation! s)
  (let ((d (doc s)))
    (move! s (lambda (h) (let ((i (indentation d h))) (if (<= h i) i h))))))

;; The starts of the lines the ranges of S are on, sorted, each once.
(define (range-lines s whole)
  (let ((d (doc s)))
    (delete-duplicates
     (sort (append-map (lambda (r)
                         (if whole
                             (line-starts d (min (car r) (cadr r)) (max (car r) (cadr r)))
                             (list (line-start d (cadr r)))))
                       (ranges s))
           <))))

(define (indent-here! s group)
  "Indent the lines of the heads of S's ranges, as the buffer's mode does.
Heads in a line's leading blanks go past them. GROUP is as `edit!`
takes it. Return #f if the mode does not indent."
  ;; The lines found only where the mode indents: a line's start is far
  ;; on a long one.
  (and (indent-lines! s (lambda () (range-lines s #f)) group)
       (begin (to-indentation! s) #t)))

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
