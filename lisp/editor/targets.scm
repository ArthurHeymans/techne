;;; Targets (EDITOR.md, sections 1 and 10): what a candidate or the text at
;;; point stands for (a file, a buffer, a command, a location), and the
;;; actions on each kind. Embark is the reference: whatever produces
;;; targets of a kind gets all its actions, from the minibuffer and at
;;; point, without code of its own.
;;;
;;; Actions live in a registry, owned by the scope that defines them; the
;;; first defined for a kind is its default, which RET takes.
;;;
;;; A location is a position in a document as of a revision, or a line of
;;; a file, whose document is opened when it is needed. Files are opened
;;; once: a file visited again is the same document.

(require "session.scm")
(require "modes.scm")
(require "commands.scm")

(provide target target? target-kind target-value
         define-action register-action! actions-for action-name action-doc run-action! act-default!
         location file-location location? location-document location-position
         line-candidate-text target-at file-document absolute-path directory-of file-name working-directory)

(define-record-type target
  (target kind value)
  target?
  (kind target-kind)
  (value target-value))

;;; Actions

(define-record-type action
  (make-action kind name doc proc order)
  action?
  (kind action-kind)
  (name action-name)
  (doc action-doc)
  ;; (session value) -> anything
  (proc action-proc)
  (order action-order))

(define %actions (make-registry 'actions))
(define %action-count 0)

;; Define the action NAME on targets of KIND: (PROC session value).
(define (register-action! kind name doc proc)
  (let ((old (registry-ref %actions (list kind name))))
    (set! %action-count (+ %action-count 1))
    ;; A redefinition keeps its place.
    (registry-add! %actions (list kind name)
                   (make-action kind name doc proc (if old (action-order old) %action-count)))
    name))

;; (define-action kind (name s value) doc body ...): the procedure is
;; called through its global binding, so redefining it takes effect at once.
(define-syntax define-action
  (syntax-rules ()
    ((_ kind (name s v) doc body ...)
     (begin
       (define (name s v) body ...)
       (register-action! 'kind 'name doc (lambda (s2 v2) (name s2 v2)))))))

;; The actions on targets of KIND, the default first.
(define (actions-for kind)
  (sort (filter-map (lambda (k) (and (eq? (car k) kind) (registry-ref %actions k))) (registry-keys %actions))
        (lambda (a b) (< (action-order a) (action-order b)))))

(define (run-action! s action t) ((action-proc action) s (target-value t)))

;; Do the default action on target T.
(define (act-default! s t)
  (let ((actions (actions-for (target-kind t))))
    (if (null? actions)
        (error "No action on a target of this kind" (target-kind t))
        (run-action! s (car actions) t))))

;;; Files

(define (last-slash path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref path i) #\/) i)
          (else (loop (- i 1))))))

;; The directory part of a path, with its slash: "" when there is none.
(define (directory-of path)
  (let ((i (last-slash path))) (if i (substring path 0 (+ i 1)) "")))

(define (file-name path) (substring path (string-length (directory-of path)) (string-length path)))

(define (home) (or (get-environment-variable "HOME") "/"))
(define (working-directory) (or (get-environment-variable "PWD") (home)))

(define (absolute-path path)
  (cond ((string-prefix? "/" path) path)
        ((string-prefix? "~/" path) (string-append (home) (substring path 1 (string-length path))))
        (else (string-append (working-directory) "/" path))))

(define %documents (make-hash-table))

;; The document of the file at PATH, opened with its journal the first
;; time; a document already open (D) is registered as its file's.
(define (file-document path #:document [d #f])
  (let ((path (absolute-path path)))
    (or (hash-table-ref/default %documents path #f)
        (let ((d (or d (open-file path))))
          (hash-table-set! %documents path d)
          d))))

;;; Locations

(define-record-type location
  (%location place position revision)
  location?
  ;; A document, or a file's name.
  (place location-place)
  ;; In a document a position as of `revision`; in a file a line.
  (position %location-position)
  (revision location-revision))

(define (location d pos) (%location d pos (document-revision d)))

;; Line LINE (from 1) of the file at PATH.
(define (file-location path line) (%location path line #f))

(define (location-document l)
  (let ((p (location-place l)))
    (if (string? p) (file-document p) p)))

;; Where the location is now: its position mapped past the edits since.
(define (location-position l)
  (let ((d (location-document l)))
    (if (string? (location-place l))
        (line-down d 0 (- (%location-position l) 1) 0)
        (or (document-map-position d (%location-position l) (location-revision l))
            (error "The history of this location's document is gone")))))

;; The line at POS in D, as candidate text: the line itself.
(define (line-candidate-text d pos)
  (document-substring d (line-start d pos) (line-end d pos)))

;; The target at POS in a document, if its buffer's mode finds them.
(define (target-at d pos)
  (let ((b (document-buffer d)))
    (and b (buffer-target-at b pos))))
