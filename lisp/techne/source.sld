;;; Headless source syntax, backed by the editor's shared parser engine.
(define-library (techne source)
 (export source-parse source-query source-node source-text source-parent source-previous source-complete?)
 (import (techne))
 (begin
  (define (source-parse grammar text)
    "Parse TEXT in GRAMMAR, returning an immutable syntax tree.
GRAMMAR is a built-in grammar name, a string. Text is at most 1 MiB;
parsing has a two-second budget. This does not open an editor or a file."
    (%source-parse grammar text))
  (define (source-query tree query)
    "Query TREE with tree-sitter QUERY, returning captures and completeness.
Items are (capture start end), in Unicode character coordinates. The
query text is at most 16 KiB; execution has a two-second budget and at
most 20000 captures. Syntax errors
make complete false; cancellation and exhausted limits raise errors."
    (%source-query tree query))
  (define (source-node tree from to)
    "Return TREE's smallest named node holding FROM to TO.
Nodes are (kind start end named?), in Unicode character coordinates."
    (%source-node tree from to))
  (define (source-text tree from to)
    "Read character range FROM to TO of TREE.
The rope avoids rescanning UTF-8 prefixes."
    (%source-text tree from to))
  (define (source-parent tree node)
    "Return NODE's parent in TREE, or #f."
    (%source-relative tree (car node) (cadr node) (caddr node) #t))
  (define (source-previous tree node)
    "Return NODE's previous named sibling in TREE, or #f."
    (%source-relative tree (car node) (cadr node) (caddr node) #f))
  (define (source-complete? tree)
    "Return whether TREE parsed without syntax errors."
    (%source-complete? tree))))
