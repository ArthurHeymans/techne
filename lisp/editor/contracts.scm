;;; Commands with contracts (doc/operations.md, "Contracts"): what the
;;; editor's own kinds of values are as data, how a command's arguments
;;; are read from the user, and its contract as JSON Schema for agents.
;;;
;;; A command with a contract (`define-command` with #:arguments) is run
;;; from keys by reading its arguments, then invoking it with them
;;; (`invoke`), which validates them and runs it as an operation; an agent
;;; invokes it with the same arguments as data. Reading follows the
;;; command's #:read, else each argument's schema: what is taken from the
;;; context (point, the region, the prefix argument) is taken first, as
;;; values, so that a prompt ending after the focus moved acts on what was
;;; focused when the command ran; the prompts come after, in order, each
;;; knowing the arguments before it.
;;;
;;; Kinds: a `location` (a place in a document as of a revision), a
;;; `range` (a span of one), a `document` (as of a revision, if said), a
;;; `file` (a path) and a `buffer`.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "emacs.scm")
(require "../schema.scm")

(provide text-range text-range? text-range-document text-range-from text-range-to text-range-revision
         text-range-text command-schema exposed-command-schemas define-reader! command-available)

;;; Kinds

(define (field d key)
  (cond ((not (dict? d)) #f)
        ((dict-contains? d key) (dict-ref d key))
        ((dict-contains? d (symbol->string key)) (dict-ref d (symbol->string key)))
        (else #f)))

;; A document named in data: a file's, opened if need be, or a buffer's.
(define (named-document data)
  (let ((file (field data 'file)) (buffer (field data 'buffer)))
    (cond ((string? file) (file-document (absolute-path file)))
          ((string? buffer) (let ((b (buffer-named buffer)))
                              (if b (buffer-document b) (error (string-append "No buffer is called " buffer)))))
          (else (error "Not a file or buffer: give \"file\" or \"buffer\"")))))

;; The position POS of D as of REVISION mapped to now, or an error
;; saying that revision is gone: a reference is stale, never guessed.
(define (mapped d pos revision)
  (or (document-map-position d pos revision)
      (error "That revision of the text is gone: it changed too much since")))

;; Where D is, as data: its file, else its buffer.
(define (document-data d)
  (cond ((document-path d) (dict "file" (document-path d)))
        ((document-buffer d) => (lambda (b) (dict "buffer" (buffer-name b))))
        (else (error "This document is in no file or buffer"))))

(define-schema-kind 'location location? "a place in a document"
  #:from-data (lambda (v)
                (let ((d (named-document v)) (line (field v 'line)) (column (field v 'column)) (position (field v 'position)))
                  (cond ((and (exact-integer? position) (exact-integer? (field v 'revision)))
                         ;; As of a revision: mapped through the changes since.
                         (location d (mapped d position (field v 'revision))))
                        ((and (exact-integer? line) (>= line 1))
                         (if (document-path d)
                             (file-location (document-path d) line #:column (if (exact-integer? column) column 1))
                             (location d (line-down d 0 (- line 1) (- (if (exact-integer? column) column 1) 1)))))
                        ((exact-integer? position) (location d position))
                        (else (error "Not a place: give \"line\" (and \"column\"), or \"position\"")))))
  ;; Its line and column now, for people; its position as of its
  ;; revision, which reading it back maps through the changes since.
  #:to-data (lambda (l)
              (let* ((d (location-document l)) (pos (location-position l))
                     (data (dict-set (dict-set (document-data d) "line" (line-number d pos)) "column" (+ 1 (column d pos)))))
                (if (location-revision l)
                    (dict-set (dict-set data "position" (location-offset l)) "revision" (location-revision l))
                    data)))
  #:json (dict "type" "object"
               "description" "A place in a file (or a buffer): its line, and column, from 1."
               "properties" (dict "file" (dict "type" "string") "buffer" (dict "type" "string")
                                  "line" (dict "type" "integer" "minimum" 1) "column" (dict "type" "integer" "minimum" 1)
                                  "position" (dict "type" "integer" "minimum" 0) "revision" (dict "type" "integer" "minimum" 0))
               "required" (vector "line")))

;; A span of a document as of a revision, as the region was when read.
(define-record-type text-range
  (text-range document from to revision)
  text-range?
  (document text-range-document)
  (from %text-range-from)
  (to %text-range-to)
  (revision text-range-revision))

(define (text-range-from r)
  "Return where the start of range R is now."
  (or (document-map-position (text-range-document r) (%text-range-from r) (text-range-revision r))
      (error "The history of this range's document is gone")))

(define (text-range-to r)
  "Return where the end of range R is now."
  (or (document-map-position (text-range-document r) (%text-range-to r) (text-range-revision r))
      (error "The history of this range's document is gone")))

(define (text-range-text r)
  "Return the text of range R as it is now."
  (document-substring (text-range-document r) (text-range-from r) (text-range-to r)))

(define-schema-kind 'range text-range? "a span of a document"
  #:from-data (lambda (v)
                (let ((d (named-document v)) (from (field v 'from)) (to (field v 'to)))
                  (unless (and (exact-integer? from) (exact-integer? to) (<= 0 from to))
                    (error "Not a span: give \"from\" and \"to\", positions in it"))
                  (let ((revision (field v 'revision)))
                    (if (exact-integer? revision)
                        ;; As of a revision: its ends mapped, or it is stale.
                        (begin (mapped d from revision) (mapped d to revision) (text-range d from to revision))
                        (begin (unless (<= to (document-length d)) (error "Not a span of it: it is shorter"))
                               (text-range d from to (document-revision d)))))))
  ;; Its ends as of its revision, which reading it back maps.
  #:to-data (lambda (r)
              (let ((d (text-range-document r)))
                (dict-set (dict-set (dict-set (document-data d) "from" (%text-range-from r)) "to" (%text-range-to r))
                          "revision" (text-range-revision r))))
  #:json (dict "type" "object" "description" "A span of a file (or a buffer): its positions, from and to."
               "properties" (dict "file" (dict "type" "string") "buffer" (dict "type" "string")
                                  "from" (dict "type" "integer" "minimum" 0) "to" (dict "type" "integer" "minimum" 0)
                                  "revision" (dict "type" "integer" "minimum" 0))
               "required" (vector "from" "to")))

(define-schema-kind 'document document? "a document"
  ;; With a revision, the document as it was then: refused, stale, once
  ;; it has changed since.
  #:from-data (lambda (v)
                (let ((d (named-document v)) (revision (field v 'revision)))
                  (when (and (exact-integer? revision) (not (= revision (document-revision d))))
                    (error (string-append "it changed since: revision " (number->string revision)
                                          " then, " (number->string (document-revision d)) " now")))
                  d))
  #:to-data (lambda (d) (dict-set (document-data d) "revision" (document-revision d)))
  #:json (dict "type" "object"
               "description" "A file's document (or a buffer's); with a revision, as it was then, refused if it changed since."
               "properties" (dict "file" (dict "type" "string") "buffer" (dict "type" "string")
                                  "revision" (dict "type" "integer" "minimum" 0))))

(define-schema-kind 'file string? "a file's path"
  #:json (dict "type" "string" "description" "A file's path."))

(define-schema-kind 'buffer buffer? "a buffer"
  #:from-data (lambda (v) (or (and (string? v) (buffer-named v)) (error "No buffer is called that")))
  #:to-data buffer-name
  #:json (dict "type" "string" "description" "A buffer's name."))

;;; Reading arguments

;; Readers by name: (session entry options args k) for the prompts, which
;; call (K value) once read; for the context, (session entry options args
;; prefix) giving the value.
(define %prompts (make-hash-table))
(define %contexts (make-hash-table))

(define (define-reader! name proc #:context [context #f])
  "Make NAME a way to read an argument, for a command's #:read.
PROC is (session entry options args k): ENTRY the argument's (name
schema option ...), OPTIONS the #:read entry's options, ARGS a dict of
the arguments read before; it calls (K value) once read. With CONTEXT,
PROC is (session entry options args prefix) and gives the value at
once, from the context, before any prompt."
  (hash-table-set! (if context %contexts %prompts) name proc))

(define (option-of options key default)
  (let loop ((o options))
    (cond ((or (null? o) (null? (cdr o))) default)
          ((eq? (car o) key) (cadr o))
          (else (loop (cddr o))))))

;; An option that may be a procedure of the session and the arguments.
(define (option-value s options key args default)
  (let ((v (option-of options key default)))
    (if (procedure? v) (v s args) v)))

(define (prompt-of s e options args)
  (option-value s options #:prompt args
                (string-append (string-upcase (substring (symbol->string (car e)) 0 1))
                               (substring (symbol->string (car e)) 1 (string-length (symbol->string (car e))))
                               ": ")))

(define (initial-of s options args)
  (let ((v (option-value s options #:initial args "")))
    (if (string? v) v (call-with-output-string (lambda (p) (write v p))))))

(define-reader! 'prompt
  (lambda (s e options args k)
    (completing-read s (prompt-of s e options args) '()
                     #:initial (initial-of s options args) #:require-match #f
                     #:history (option-of options #:history #f) #:remember 'input
                     #:accept (lambda (s c) (k (candidate-text c))))))

(define-reader! 'number
  (lambda (s e options args k)
    (completing-read s (prompt-of s e options args) '()
                     #:initial (initial-of s options args) #:require-match #f
                     #:accept (lambda (s c) (k (or (string->number (candidate-text c)) (candidate-text c)))))))

;; One of the schema's choices, or of #:choices (a list or a procedure
;; of the session and the arguments); a datum read back when not.
(define (choices-of s e options args)
  (let ((given (option-value s options #:choices args #f)))
    (or given (schema-choices (entry-schema* e args)))))

(define (entry-schema* e args) (if (procedure? (cadr e)) ((cadr e) args) (cadr e)))

(define (written v) (if (string? v) v (call-with-output-string (lambda (p) (write v p)))))

(define-reader! 'choice
  (lambda (s e options args k)
    (let* ((choices (choices-of s e options args))
           (annotate (option-of options #:annotate #f)))
      (completing-read s (prompt-of s e options args)
                       (map (lambda (c) (candidate (written c) #:annotation (if annotate (annotate c) ""))) choices)
                       #:initial (initial-of s options args)
                       #:require-match #f
                       #:accept (lambda (s c)
                                  (let* ((text (candidate-text c))
                                         (found (find (lambda (x) (equal? (written x) text)) choices)))
                                    (k (if (or found (member text choices))
                                           (or found text)
                                           (guard (x (#t text)) (read (open-input-string text)))))))))))

(define-reader! 'yes-or-no
  (lambda (s e options args k)
    (completing-read s (prompt-of s e options args) '("yes" "no")
                     #:accept (lambda (s c) (k (string=? (candidate-text c) "yes"))))))

(define-reader! 'file
  (lambda (s e options args k)
    (read-file-name s (prompt-of s e options args)
                    (option-value s options #:initial args (abbreviate-path (default-directory s)))
                    (lambda (s path) (k path)))))

;; From the context: where point is, the region, the prefix argument, the
;; buffer, or what #:value (session args) gives.

(define-reader! 'point
  (lambda (s e options args prefix) (location (doc s) (point s)))
  #:context #t)

(define-reader! 'region
  (lambda (s e options args prefix)
    (let* ((v (session-view s)) (r (list-ref (view-ranges v) (view-primary v))))
      (text-range (view-document v) (min (car r) (cadr r)) (max (car r) (cadr r)) (document-revision (view-document v)))))
  #:context #t)

(define-reader! 'prefix
  (lambda (s e options args prefix)
    (let ((head (let ((sc (cadr e))) (if (pair? sc) (car sc) sc))))
      (if (memq head '(integer natural number))
          (prefix-count prefix)
          (and prefix #t))))
  #:context #t)

(define-reader! 'document-here
  (lambda (s e options args prefix) (view-document (session-view s)))
  #:context #t)

(define-reader! 'buffer-here
  (lambda (s e options args prefix) (current-buffer s))
  #:context #t)

(define-reader! 'context
  (lambda (s e options args prefix) ((option-of options #:value (lambda (s args) #f)) s args))
  #:context #t)

;; How to read argument E of a contract, by its #:read entry, else its
;; schema: (how . options).
(define (reading contract e)
  (let ((spec (assq (car e) (contract-read contract))))
    (if spec
        (cdr spec)
        (let* ((sc (cadr e)) (head (cond ((symbol? sc) sc) ((pair? sc) (car sc)) (else #f))))
          (list (case head
                  ((location) 'point)
                  ((range) 'region)
                  ((file) 'file)
                  ((buffer) 'buffer-here)
                  ((document) 'document-here)
                  ((boolean) 'yes-or-no)
                  ((one-of) 'choice)
                  ((integer natural number real) 'number)
                  (else 'prompt)))))))

(define (context-reader? how) (and (symbol? how) (hash-table-ref/default %contexts how #f)))

;; Read the arguments of command NAME in session S, then invoke it with
;; them: what the context gives first, then the prompts, in order.
(define (acquire! s name n)
  (let* ((contract (command-contract name))
         (prefix (current-prefix s))
         (entries (contract-entries contract))
         ;; Each argument conformed as read, as `invoke` would: the readers
         ;; after it get it as it will be (a symbol, not the string typed).
         (conformed (lambda (e v args) (contract-conform-argument name (car e) v args)))
         (args (guard (x (#t (failed! s name x) #f))
                 (fold (lambda (e args)
                         (let* ((r (reading contract e)) (how (car r)))
                           (if (context-reader? how)
                               (dict-set args (car e) (conformed e ((context-reader? how) s e (cdr r) args prefix) args))
                               args)))
                       (dict) entries))))
    (let ask ((entries entries) (args args))
      (cond ((not args) #f)
            ((null? entries)
             (guard (e (#t (failed! s name e)))
               (invoke s name args)))
            ((dict-contains? args (car (car entries))) (ask (cdr entries) args))
            (else
             (let* ((e (car entries)) (r (reading contract e)) (how (car r))
                    (k (lambda (v)
                         (let ((v (guard (x (#t (failed! s name x) x)) (conformed e v args))))
                           ;; Refused: said, and nothing more is read.
                           (unless (error-object? v)
                             (ask (cdr entries) (dict-set args (car e) v)))))))
               (cond ((procedure? how) (how s args k))
                     ((hash-table-ref/default %prompts how #f) => (lambda (read) (read s e (cdr r) args k)))
                     (else (failed! s name (guard (x (#t x)) (error "No way to read this argument:" (car e))))))))))))

(set-command-acquirer! acquire!)

;;; For agents

(define (command-available s name)
  "Return #t if the command NAME can run in session S, else why not."
  (let* ((contract (command-contract name)) (available (and contract (contract-available contract))))
    (if available (available s) #t)))

(define (command-schema name)
  "Return the command NAME's contract as data for JSON, for an agent.
A dict: its name, description, input (the JSON Schema of its
arguments), effects and, when it has one, its result's schema."
  (let ((contract (or (command-contract name) (error "This command has no contract" name))))
    (let ((d (dict "name" (symbol->string name)
                   "description" (or (command-doc name) "")
                   "input" (schema->json-schema (contract-argument-schema contract))
                   "effects" (let ((e (contract-effects contract)))
                               (if (procedure? e) "depend on the arguments" (list->vector (map symbol->string e)))))))
      (if (contract-result contract)
          (dict-set d "result" (schema->json-schema (contract-result contract)))
          d))))

(define (exposed-command-schemas)
  "Return the contracts of the commands exposed to agents, by name.
Each as `command-schema` gives it."
  (map command-schema
       (sort (filter (lambda (name) (let ((c (command-contract name))) (and c (memq 'agents (contract-expose c)))))
                     (command-names))
             (lambda (a b) (string<? (symbol->string a) (symbol->string b))))))
