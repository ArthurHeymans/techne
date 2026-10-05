;; Embark-style actions over typed objects, with recoverable errors and
;; background tasks.

(define-record-type file-ref (make-file-ref path) file-ref? (path file-path))
(define-record-type commit (make-commit id message) commit? (id commit-id) (message commit-message))

;; Which actions apply to an object is a generic function: new types (also
;; Rust types via `name_foreign_type`) just add methods.
(define-generic (actions x))
(define-method (actions x) '(inspect))
(define-method (actions (f file-ref)) '(open rename delete inspect))
(define-method (actions (c commit)) '(show revert inspect))
(define-method (actions (s string)) '(copy search inspect))

(define (describe x)
  (match x
    [(file-ref p) (string-append "file " p)]
    [(commit id msg) #:when (> (string-length msg) 20) (string-append "commit " id " (long message)")]
    [(commit id msg) (string-append "commit " id ": " msg)]
    [(? string? s) (string-append "text \"" s "\"")]
    [_ (repr x)]))

(define (run-action action target #:dry-run [dry-run #f])
  (restart-case
   (if (and (eq? action 'delete) (not dry-run))
       (error "refusing to delete without confirmation:" (describe target))
       (string-append (symbol->string action) " -> " (describe target)))
   (dry-run () (run-action action target #:dry-run #t))
   (skip () 'skipped)))

(define targets (list (make-file-ref "/tmp/notes.org") (make-commit "a1b2" "Fix timer") "hello" 42))

;; Errors become a choice of restarts: here an automatic policy picks one.
(define results
  (handler-bind ((error-object? (lambda (c) (invoke-restart 'dry-run))))
    (map (lambda (t) (map (lambda (a) (run-action a t)) (actions t))) targets)))
(for-each displayln (apply append results))

;; Indexing every target concurrently in background tasks.
(define index (make-channel))
(for-each (lambda (t) (spawn (lambda () (sleep 5) (channel-send index (cons (describe t) (length (actions t)))))))
          targets)
(displayln (sort (map (lambda (_) (channel-recv index)) targets) (lambda (a b) (string<? (car a) (car b)))))
