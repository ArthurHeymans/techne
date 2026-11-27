;;; Targets (EDITOR.md, sections 1 and 10): what a candidate or the text at
;;; point stands for (a file, a buffer, a command, a location), and the
;;; actions on each kind. Embark is the reference: whatever produces
;;; targets of a kind gets all its actions, from the minibuffer and at
;;; point, without code of its own.
;;;
;;;; Actions live in a registry, owned by the scope that defines them, and
;;; run in it; the first defined for a kind is its default, which RET takes.
;;;
;;; A location is a position in a document as of a revision, or a line of
;;; a file, whose document is opened when it is needed (files.scm).

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "files.scm")

(provide target target? target-kind target-value
         define-action register-action! register-lazy-actions! actions-for all-actions action-kind action-name action-doc action-key action-interactive? run-action! act-default!
         location file-location location? location-document location-position location-revision location-offset
         line-candidate-text target-at set-fallback-target-at!)

(define-record-type target
  (target kind value)
  target?
  (kind target-kind)
  (value target-value))

;;; Actions

(define-record-type action
  (make-action kind name doc proc order key interactive?)
  action?
  (kind action-kind)
  (name action-name)
  (doc action-doc)
  ;; (session value) -> anything
  (proc action-proc)
  (order action-order)
  ;; Optional single-key binding string (e.g. "d", "r", "o") for quick dispatch.
  (key action-key)
  ;; Whether the action prompts interactively (excluded from batch act-all).
  (interactive? action-interactive?))

(define %actions (make-registry 'actions))
(define %action-count 0)

(define (register-action! kind name doc proc #:key [key #f] #:default [default? #f] #:interactive [interactive? #f])
  "Define the action NAME on targets of KIND, documented by DOC.
PROC is called as (PROC session value). Optional KEY names a single-key
shortcut; DEFAULT? pins it first regardless of load order; INTERACTIVE?
marks prompting actions excluded from batch dispatch."
  (let ((old (registry-ref %actions (list kind name))))
    (set! %action-count (+ %action-count 1))
    ;; A redefinition keeps its place; an explicit default always sorts first.
    (registry-add! %actions (list kind name)
                   (make-action kind name doc (scope-procedure proc)
                                (cond (default? 0)
                                      (old (action-order old))
                                      (else %action-count))
                                (or key (and old (action-key old)))
                                (or interactive? (and old (action-interactive? old)))))
    name))

(define-syntax %define-action
  (syntax-rules ()
    ((_ kind sig (opt ...) #:key k rest ...) (%define-action kind sig (opt ... #:key k) rest ...))
    ((_ kind sig (opt ...) #:default d rest ...) (%define-action kind sig (opt ... #:default d) rest ...))
    ((_ kind sig (opt ...) #:interactive i rest ...) (%define-action kind sig (opt ... #:interactive i) rest ...))
    ((_ kind (name s v) (opt ...) doc body ...)
     (begin
       (define (name s v) doc body ...)
       (register-action! 'kind 'name doc (lambda (s2 v2) (name s2 v2)) opt ...)))))

;; The procedure is called through its global binding, so redefining it
;; takes effect at once.
(define-syntax define-action
  (syntax-rules ()
    "Define the action NAME on targets of KIND.
It is a procedure of the session S and the target's value V; DOC
documents both the procedure and the action. Supports optional `#:key`,
`#:default`, and `#:interactive` flags before DOC."
    ((_ kind sig rest ...) (%define-action kind sig () rest ...))))

(define %lazy-action-loaders '())

(define (register-lazy-actions! kinds loader)
  "Register LOADER to run once before `actions-for` returns KINDS."
  (set! %lazy-action-loaders (cons (cons kinds (scope-procedure loader)) %lazy-action-loaders)))

(define (ensure-lazy-actions! kind)
  (let ((run (filter (lambda (e) (memq kind (car e))) %lazy-action-loaders)))
    (when (pair? run)
      (set! %lazy-action-loaders (filter (lambda (e) (not (memq kind (car e)))) %lazy-action-loaders))
      (for-each (lambda (entry)
                  (guard (e (#t (set! %lazy-action-loaders (cons entry %lazy-action-loaders))
                                (unless (string-contains (error-text e) "capability") (raise e))))
                    ((cdr entry))))
                (reverse run)))))

(define (actions-for kind)
  "Return the actions on targets of KIND, the default first."
  (ensure-lazy-actions! kind)
  (sort (filter-map (lambda (k) (and (eq? (car k) kind) (registry-ref %actions k))) (registry-keys %actions))
        (lambda (a b) (< (action-order a) (action-order b)))))

(define (run-action! s action t)
  "Do ACTION on the target T in session S."
  ((action-proc action) s (target-value t)))

(define (all-actions)
  "Return every action, of every kind."
  (filter-map (lambda (k) (registry-ref %actions k)) (registry-keys %actions)))

(define (act-default! s t)
  "Do the default action on target T in session S."
  (let ((actions (actions-for (target-kind t))))
    (if (null? actions)
        (error "No action on a target of this kind" (target-kind t))
        (run-action! s (car actions) t))))

;;; Locations

(define-record-type location
  (%location place position revision column)
  location?
  ;; A document, or a file's name.
  (place location-place)
  ;; In a document a position as of `revision`; in a file a line, and
  ;; the column in it.
  (position %location-position)
  (revision location-revision)
  (column location-column))

(define (location d pos #:revision [revision (document-revision d)])
  "Return the location of POS in document D, as of REVISION.
REVISION is the document's revision now unless given."
  (%location d pos revision #f))

(define (file-location path line #:column [column 1])
  "Return the location of line LINE of the file at PATH, at COLUMN.
Both count from 1."
  (%location path line #f column))

(define (location-document l)
  "Return the document of the location L, opening its file if needed."
  (let ((p (location-place l)))
    (if (string? p) (file-document p) p)))

(define (location-offset l)
  "Return the position of location L as of its revision, not mapped.
For a file's location, its line."
  (%location-position l))

(define (location-position l)
  "Return where the location L is now, mapped past the edits since."
  (let ((d (location-document l)))
    (if (string? (location-place l))
        (line-down d 0 (- (%location-position l) 1) (- (location-column l) 1))
        (or (document-map-position d (%location-position l) (location-revision l))
            (error "The history of this location's document is gone")))))

(define (line-candidate-text d pos)
  "Return the line at POS in D, as candidate text."
  (document-substring d (line-start d pos) (line-end d pos)))

(define %fallback-target-at #f)

(define (set-fallback-target-at! proc)
  "Register PROC as the fallback target detector for `target-at`."
  (set! %fallback-target-at proc))

(define (target-at d pos)
  "Return the target at POS in document D, or #f.
It is found by D's buffer's mode, or falls back to a URL or existing
file path at POS."
  (let ((b (document-buffer d)))
    (or (and b (buffer-target-at b pos))
        (begin
          (unless %fallback-target-at (ensure-lazy-actions! 'file))
          (and %fallback-target-at (%fallback-target-at d pos))))))
