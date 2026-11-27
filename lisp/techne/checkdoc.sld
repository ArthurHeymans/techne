;;; Core documentation checker for Techne Scheme modules and libraries.
;;;
;;; Works in any world (plain VM, standalone apps, or the editor): checks
;;; that exported procedures, macros, and variables have docstrings that
;;; follow Techne's docstring conventions. Editor-specific checks (commands,
;;; keymaps, modes, options, hooks) live in `lisp/editor/checkdoc.scm` and
;;; build on this library.

(define-library (techne checkdoc)
  (export problem? make-doc-problem
          problem-module problem-subject problem-name problem-text problem-location
          format-problem
          check-docstring module-documentation-problems documentation-problems)
  (import (techne))
  (begin

    (define-record-type doc-problem
      (%make-doc-problem module subject name text location)
      problem?
      (module problem-module)
      (subject problem-subject)
      (name problem-name)
      (text problem-text)
      (location problem-location))

    (define (make-doc-problem module subject name text location)
      "Construct a documentation problem record for NAME in MODULE.
SUBJECT names the kind of binding, TEXT explains the issue, and
LOCATION is `(file line col)` or `#f`."
      (%make-doc-problem module subject name text location))

    (define (format-problem p #:strip-prefix [strip-prefix #f])
      "Format documentation problem P as a single human-readable line.
When STRIP-PREFIX is a string, removes it from quoted paths in the output."
      (let* ((mod (repr (problem-module p)))
             (mod (if (and (string? strip-prefix) (string-prefix? strip-prefix mod))
                      (string-append "\"" (substring mod (string-length strip-prefix) (string-length mod)))
                      mod)))
        (string-append mod " " (symbol->string (problem-subject p))
                       " " (symbol->string (problem-name p)) ": " (problem-text p))))

    (define (internal? name)
      (string-prefix? "%" (symbol->string name)))

    (define (field alist key)
      (let ((e (assq key alist))) (and e (cdr e))))

    (define (check-docstring module subject name doc params location #:known-commands [known-commands #f])
      "Check DOC for SUBJECT named NAME in MODULE with PARAMS at LOCATION.
When KNOWN-COMMANDS is a list of command symbols, also verifies `\\[cmd]`
key references in DOC."
      (let ((rule (case subject
                    ((procedure built-in) 'procedure)
                    ((command action) 'command)
                    (else 'other))))
        (map (lambda (text) (%make-doc-problem module subject name text location))
             (if (string? doc)
                 (append (docstring-problems doc rule (map (lambda (p) (if (symbol? p) (symbol->string p) p)) params))
                         (if known-commands
                             (filter-map (lambda (c)
                                           (and (not (memq c known-commands))
                                                (string-append "\\[" (symbol->string c) "] names no command.")))
                                         (docstring-key-references doc))
                             '()))
                 (list "Document it with a docstring.")))))

    (define (binding-problems module name known-commands)
      (let ((d (binding-description name module)))
        (if (not d)
            (list (%make-doc-problem module 'binding name "It is provided but not defined." #f))
            (let ((kind (field d 'kind)))
              (check-docstring module
                               (cond ((eq? kind 'procedure) 'procedure)
                                     ((eq? kind (string->symbol "built-in procedure")) 'built-in)
                                     ((eq? kind 'macro) 'macro)
                                     (else 'variable))
                               name (field d 'doc) (or (field d 'params) '()) (field d 'location)
                               #:known-commands known-commands)))))

    (define (module-documentation-problems module #:skip [skip? #f] #:known-commands [known-commands #f])
      "Return documentation problems for public bindings defined in MODULE.
MODULE is a module name string or R7RS library name list; for `\"root\"`,
checks every non-internal definition. Optional SKIP? excludes bindings
(such as editor commands checked separately), and KNOWN-COMMANDS
validates `\\[cmd]` references."
      (let* ((mod (if (string? module) module (repr module)))
             (own (module-definitions mod))
             (names (filter (lambda (n) (memq n own))
                            (or (module-exports mod) (if (equal? mod "root") own '())))))
        (append-map (lambda (name) (binding-problems mod name known-commands))
                    (remove (lambda (n) (or (internal? n) (and skip? (skip? n))))
                            (delete-duplicates names)))))

    (define (documentation-problems [modules (loaded-modules)] #:skip [skip? #f] #:known-commands [known-commands #f])
      "Return documentation problems across MODULES (default: loaded modules).
SKIP? and KNOWN-COMMANDS are forwarded to `module-documentation-problems`."
      (append-map (lambda (m) (module-documentation-problems m #:skip skip? #:known-commands known-commands))
                  modules))))
