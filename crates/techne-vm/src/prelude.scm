;;; Core library written in Scheme. Higher-order procedures live here rather
;;; than in Rust so that they call closures through the ordinary VM path
;;; (proper tail calls, resumable later). Derived syntax is defined with
;;; syntax-rules.

;; ----- control (no native callbacks, so tasks can suspend inside) -----

(define (call/cc f) (%with-escape f))
(define call-with-current-continuation call/cc)
(define call/ec call/cc)

(define (dynamic-wind before thunk after)
  (before)
  (%push-wind after)
  (let ((result (thunk)))
    (%pop-handler)
    (after)
    result))

(define (with-exception-handler handler thunk)
  (%push-handler handler)
  (let ((result (thunk)))
    (%pop-handler)
    result))

(define (call-with-values producer consumer)
  (let ((v (producer)))
    (if (%values? v) (apply consumer (%values->list v)) (consumer v))))

;; ----- syntax -----

(define-syntax do
  (syntax-rules ()
    ((_ ((var init step ...) ...) (test expr ...) command ...)
     (let loop ((var init) ...)
       (if test
           (begin (void) expr ...)
           (begin command ... (loop (do "step" var step ...) ...)))))
    ((_ "step" x) x)
    ((_ "step" x y) y)))

(define-syntax case-lambda
  (syntax-rules ()
    ((_ (formals body ...) ...)
     (lambda args
       (%case-lambda-dispatch args (formals body ...) ...)))))

(define-syntax %case-lambda-dispatch
  (syntax-rules ()
    ((_ args) (error "case-lambda: no clause matches" args))
    ((_ args ((p ...) body ...) clause ...)
     (if (= (length args) (length '(p ...)))
         (apply (lambda (p ...) body ...) args)
         (%case-lambda-dispatch args clause ...)))
    ((_ args ((p ... . rest) body ...) clause ...)
     (if (>= (length args) (length '(p ...)))
         (apply (lambda (p ... . rest) body ...) args)
         (%case-lambda-dispatch args clause ...)))
    ((_ args (rest body ...) clause ...)
     (apply (lambda rest body ...) args))))

(define-syntax receive
  (syntax-rules ()
    ((_ formals expr body ...)
     (call-with-values (lambda () expr) (lambda formals body ...)))))

(define-syntax let-values
  (syntax-rules ()
    ((_ () body ...) (let () body ...))
    ((_ ((formals expr) rest ...) body ...)
     (call-with-values (lambda () expr)
       (lambda formals (let-values (rest ...) body ...))))))

(define-syntax let*-values
  (syntax-rules ()
    ((_ bindings body ...) (let-values bindings body ...))))

(define-syntax define-values
  (syntax-rules ()
    ((_ (var ...) expr)
     (begin
       (define var #f) ...
       (call-with-values (lambda () expr)
         (lambda vals (%set-each! vals var ...) (void)))))
    ((_ (var ... . rest) expr)
     (begin
       (define var #f) ...
       (define rest #f)
       (call-with-values (lambda () expr)
         (lambda vals (set! rest (%set-each! vals var ...))))))
    ((_ var expr)
     (define var (call-with-values (lambda () expr) list)))))

;; Sets each var to the next value; returns the values left over.
(define-syntax %set-each!
  (syntax-rules ()
    ((_ vals) vals)
    ((_ vals var rest ...) (begin (set! var (car vals)) (%set-each! (cdr vals) rest ...)))))

(define-syntax assert
  (syntax-rules ()
    ((_ expr) (unless expr (error "assertion failed:" 'expr)))))

(define-syntax delay
  (syntax-rules ()
    ((_ expr) (%make-promise #f (lambda () expr)))))

(define-syntax delay-force
  (syntax-rules ()
    ((_ expr) (%make-promise #f (lambda () expr)))))

(define-record-type promise (%make-promise done? value) promise?
  (done? %promise-done? %set-promise-done!)
  (value %promise-value %set-promise-value!))

(define (make-promise v) (if (promise? v) v (%make-promise #t v)))

(define (force p)
  (if (not (promise? p))
      p
      (if (%promise-done? p)
          (%promise-value p)
          (let ((v ((%promise-value p))))
            (if (%promise-done? p)
                (%promise-value p)
                (let ((v (if (promise? v) (force v) v)))
                  (%set-promise-done! p #t)
                  (%set-promise-value! p v)
                  v))))))

;; Parameters keep their values in task-local storage: tasks inherit the
;; values current at `spawn`, and `parameterize` in one task is invisible to
;; others. A parameter is an applicable record.
(define %parameter-type (%make-rtd 'parameter '(proc key convert) 0))

(define (%make-parameter-with-key key init convert)
  (let ((default (convert init)))
    (%record %parameter-type
             (lambda () (%task-local-ref key default))
             key
             convert)))

(define (make-parameter init . converter)
  (%make-parameter-with-key (%fresh-key) init (if (null? converter) (lambda (x) x) (car converter))))

(define (%parameter-key p) (%record-ref p %parameter-type 1))
(define (%parameter-convert p v) ((%record-ref p %parameter-type 2) v))

(define-syntax parameterize
  (syntax-rules ()
    ((_ ((param value) ...) body ...)
     (let* ((params (list param ...))
            (new (map %parameter-convert params (list value ...)))
            (old (map (lambda (p) (p)) params)))
       (dynamic-wind
        (lambda () (for-each (lambda (p v) (%task-local-set! (%parameter-key p) v)) params new))
        (lambda () body ...)
        (lambda () (for-each (lambda (p v) (%task-local-set! (%parameter-key p) v)) params old)))))))

;; The standard ports. Writing without a port goes to the VM's output
;; directly while current-output-port is not parameterized.
(define current-output-port (%make-parameter-with-key (%output-port-key) (%stdout) (lambda (x) x)))
(define %stdin (%make-stdin))
(define current-input-port (%make-parameter-with-key (%input-port-key) %stdin (lambda (x) x)))
(define current-error-port (make-parameter (%stderr)))

(define (get-environment-variables)
  (let loop ((l (%environment-variables)) (acc '()))
    (if (null? l) (reverse acc) (loop (cddr l) (cons (cons (car l) (cadr l)) acc)))))

;; A fresh isolated module for each environment, seeing only its imports.
(define (environment . sets) (%environment sets))
(define (scheme-report-environment . version) (%environment '((scheme r5rs))))
(define (null-environment . version) (%environment '()))
(define (interaction-environment) (current-module))

(define (call-with-port port proc)
  (call-with-values (lambda () (proc port))
    (lambda vals (close-port port) (apply values vals))))
(define (call-with-input-file file proc) (call-with-port (open-input-file file) proc))
(define (call-with-output-file file proc) (call-with-port (open-output-file file) proc))
(define (with-input-from-file file thunk)
  (call-with-input-file file (lambda (port) (parameterize ((current-input-port port)) (thunk)))))
(define (with-output-to-file file thunk)
  (call-with-output-file file (lambda (port) (parameterize ((current-output-port port)) (thunk)))))

(define (with-output-to-string thunk)
  (let ((port (open-output-string)))
    (parameterize ((current-output-port port)) (thunk))
    (get-output-string port)))

;; ----- match helpers -----

(define (%list-all? p l)
  (cond ((null? l) #t) ((pair? l) (and (p (car l)) (%list-all? p (cdr l)))) (else #f)))

(define-syntax match-lambda
  (syntax-rules ()
    ((_ clause ...) (lambda (x) (match x clause ...)))))

(define-syntax match-let
  (syntax-rules ()
    ((_ ((pat e) ...) body ...) (match (list e ...) ((pat ...) body ...)))))

;; ----- lists -----

(define (caar x) (car (car x)))
(define (cadr x) (car (cdr x)))
(define (cdar x) (cdr (car x)))
(define (cddr x) (cdr (cdr x)))
(define (caaar x) (car (car (car x))))
(define (caadr x) (car (car (cdr x))))
(define (cadar x) (car (cdr (car x))))
(define (caddr x) (car (cdr (cdr x))))
(define (cdaar x) (cdr (car (car x))))
(define (cdadr x) (cdr (car (cdr x))))
(define (cddar x) (cdr (cdr (car x))))
(define (cdddr x) (cdr (cdr (cdr x))))
(define (caaaar x) (car (car (car (car x)))))
(define (caaadr x) (car (car (car (cdr x)))))
(define (caadar x) (car (car (cdr (car x)))))
(define (caaddr x) (car (car (cdr (cdr x)))))
(define (cadaar x) (car (cdr (car (car x)))))
(define (cadadr x) (car (cdr (car (cdr x)))))
(define (caddar x) (car (cdr (cdr (car x)))))
(define (cadddr x) (car (cdr (cdr (cdr x)))))
(define (cdaaar x) (cdr (car (car (car x)))))
(define (cdaadr x) (cdr (car (car (cdr x)))))
(define (cdadar x) (cdr (car (cdr (car x)))))
(define (cdaddr x) (cdr (car (cdr (cdr x)))))
(define (cddaar x) (cdr (cdr (car (car x)))))
(define (cddadr x) (cdr (cdr (car (cdr x)))))
(define (cdddar x) (cdr (cdr (cdr (car x)))))
(define (cddddr x) (cdr (cdr (cdr (cdr x)))))
(define (first x) (car x))
(define (second x) (cadr x))
(define (third x) (caddr x))
(define (rest x) (cdr x))

(define (%map1 f l)
  (let loop ((l l) (acc '()))
    (if (null? l) (reverse acc) (loop (cdr l) (cons (f (car l)) acc)))))

(define (%any-null? ls)
  (cond ((null? ls) #f) ((null? (car ls)) #t) (else (%any-null? (cdr ls)))))

(define (map f l . more)
  (if (null? more)
      (%map1 f l)
      (let loop ((ls (cons l more)) (acc '()))
        (if (%any-null? ls)
            (reverse acc)
            (loop (%map1 cdr ls) (cons (apply f (%map1 car ls)) acc))))))

(define (for-each f l . more)
  (if (null? more)
      (let loop ((l l)) (when (pair? l) (f (car l)) (loop (cdr l))))
      (let loop ((ls (cons l more)))
        (unless (%any-null? ls)
          (apply f (%map1 car ls))
          (loop (%map1 cdr ls))))))

(define (filter p l)
  (let loop ((l l) (acc '()))
    (cond ((null? l) (reverse acc))
          ((p (car l)) (loop (cdr l) (cons (car l) acc)))
          (else (loop (cdr l) acc)))))

(define (remove p l) (filter (lambda (x) (not (p x))) l))

(define (partition p l)
  (let loop ((l l) (yes '()) (no '()))
    (cond ((null? l) (values (reverse yes) (reverse no)))
          ((p (car l)) (loop (cdr l) (cons (car l) yes) no))
          (else (loop (cdr l) yes (cons (car l) no))))))

(define (fold-left f acc l)
  (let loop ((acc acc) (l l)) (if (null? l) acc (loop (f acc (car l)) (cdr l)))))
(define (fold-right f acc l)
  (let loop ((l (reverse l)) (acc acc)) (if (null? l) acc (loop (cdr l) (f (car l) acc)))))
;; SRFI-1 fold: (kons elem acc)
(define (fold kons knil l)
  (let loop ((acc knil) (l l)) (if (null? l) acc (loop (kons (car l) acc) (cdr l)))))
;; Racket foldl: (proc elem acc)
(define (foldl f acc l) (fold f acc l))
(define (reduce f ridentity l)
  (if (null? l) ridentity (fold f (car l) (cdr l))))

(define (append-map f l) (apply append (map f l)))
(define (filter-map f l)
  (let loop ((l l) (acc '()))
    (if (null? l) (reverse acc)
        (let ((v (f (car l)))) (loop (cdr l) (if v (cons v acc) acc))))))
(define (find p l)
  (cond ((null? l) #f) ((p (car l)) (car l)) (else (find p (cdr l)))))
(define (find-tail p l)
  (cond ((null? l) #f) ((p (car l)) l) (else (find-tail p (cdr l)))))
(define (any p l)
  (and (pair? l) (or (p (car l)) (any p (cdr l)))))
(define (every p l)
  (let loop ((l l) (last #t))
    (if (null? l) last (let ((v (p (car l)))) (and v (loop (cdr l) v))))))
(define (count p l)
  (let loop ((l l) (n 0)) (if (null? l) n (loop (cdr l) (if (p (car l)) (+ n 1) n)))))
(define (delete x l) (remove (lambda (y) (equal? x y)) l))
(define (delete-duplicates l)
  (let loop ((l l) (acc '()))
    (cond ((null? l) (reverse acc))
          ((member (car l) acc) (loop (cdr l) acc))
          (else (loop (cdr l) (cons (car l) acc))))))
(define (last-pair l) (if (pair? (cdr l)) (last-pair (cdr l)) l))
(define (last l) (car (last-pair l)))
(define (list-index p l)
  (let loop ((l l) (i 0)) (cond ((null? l) #f) ((p (car l)) i) (else (loop (cdr l) (+ i 1))))))
(define (take l n) (if (= n 0) '() (cons (car l) (take (cdr l) (- n 1)))))
(define (drop l n) (if (= n 0) l (drop (cdr l) (- n 1))))
(define (list-copy l)
  (if (pair? l)
      (let ((head (cons (car l) '())))
        (let loop ((tail head) (l (cdr l)))
          (if (pair? l)
              (let ((next (cons (car l) '())))
                (set-cdr! tail next)
                (loop next (cdr l)))
              (begin (set-cdr! tail l) head))))
      l))
(define (list-set! l k v) (set-car! (list-tail l k) v))
(define (make-list n . fill)
  (let ((x (if (null? fill) #f (car fill))))
    (let loop ((i 0) (acc '())) (if (= i n) acc (loop (+ i 1) (cons x acc))))))
(define (list-tabulate n f)
  (let loop ((i (- n 1)) (acc '())) (if (< i 0) acc (loop (- i 1) (cons (f i) acc)))))
(define (iota n . args)
  (let ((start (if (pair? args) (car args) 0))
        (step (if (and (pair? args) (pair? (cdr args))) (cadr args) 1)))
    (let loop ((i (- n 1)) (acc '()))
      (if (< i 0) acc (loop (- i 1) (cons (+ start (* i step)) acc))))))
(define (range . args)
  (case (length args)
    ((1) (iota (car args)))
    ((2) (iota (max 0 (- (cadr args) (car args))) (car args)))
    (else (error "range: expected 1 or 2 arguments"))))

;; Stable merge sort. Accepts (sort list less?) and (sort less? list).
(define (%merge a b less?)
  (let loop ((a a) (b b) (acc '()))
    (cond ((null? a) (append (reverse acc) b))
          ((null? b) (append (reverse acc) a))
          ((less? (car b) (car a)) (loop a (cdr b) (cons (car b) acc)))
          (else (loop (cdr a) b (cons (car a) acc))))))
(define (%sort l less?)
  (let ((n (length l)))
    (if (< n 2)
        l
        (let ((half (quotient n 2)))
          (%merge (%sort (take l half) less?) (%sort (drop l half) less?) less?)))))
(define (sort a b)
  (cond ((procedure? a) (sort b a))
        ((vector? a) (list->vector (%sort (vector->list a) b)))
        (else (%sort a b))))
(define list-sort sort)

;; ----- vectors, strings -----

(define (vector-map f v . vs) (list->vector (apply map f (vector->list v) (map vector->list vs))))
(define (vector-for-each f v . vs) (apply for-each f (vector->list v) (map vector->list vs)))
(define (vector-append . vs) (list->vector (apply append (map vector->list vs))))
(define (string-map f s . ss) (list->string (apply map f (string->list s) (map string->list ss))))
(define (string-for-each f s . ss) (apply for-each f (string->list s) (map string->list ss)))
(define (string-null? s) (= (string-length s) 0))

;; ----- hash tables -----

(define (hash-table-ref/default h k d) (hash-table-ref h k d))
(define (hash-table-update! h k f . default)
  (hash-table-set! h k (f (if (null? default) (hash-table-ref h k) (hash-table-ref h k (car default))))))
(define (hash-table-update!/default h k f d) (hash-table-set! h k (f (hash-table-ref h k d))))
(define (hash-table-for-each h f)
  (for-each (lambda (kv) (f (car kv) (cdr kv))) (hash-table->alist h)))
(define hash-table-walk hash-table-for-each)
(define hash-table-size hash-table-count)

;; ----- generic functions -----
;; Single dispatch on the first argument's type, along the chain
;; record type -> record -> t, integer/float -> number -> t, pair/null -> list -> t,
;; named foreign type -> foreign -> t. A generic is an applicable record.

(define %generic-type (%make-rtd 'generic '(proc name methods) 0))

(define (%find-method methods x)
  (let loop ((key (%type-key x)))
    (and key (or (hash-table-ref methods key #f) (loop (%type-parent key))))))

(define (make-generic name)
  (let ((methods (make-hash-table)))
    (%record %generic-type
             (lambda args
               (if (null? args)
                   (error "generic function called without arguments:" name)
                   (let ((m (%find-method methods (car args))))
                     (if m
                         (apply m args)
                         (error "no applicable method:" name (car args))))))
             name
             methods)))

(define (generic? x) (%record? x %generic-type))
(define (generic-name g) (%record-ref g %generic-type 1))
(define (add-method! g type proc) (hash-table-set! (%record-ref g %generic-type 2) type proc))
(define (find-method g x) (%find-method (%record-ref g %generic-type 2) x))
;; Does `g` have a method for `x` (including the default)?
(define (applicable? g x) (if (find-method g x) #t #f))

(define-syntax define-generic
  (syntax-rules ()
    ((_ (name arg ...)) (define name (make-generic 'name)))
    ((_ name) (define name (make-generic 'name)))))

(define-syntax define-method
  (syntax-rules ()
    ((_ (name (arg type) param ...) body ...)
     (add-method! name (%type-designator 'type (lambda () type)) (lambda (arg param ...) body ...)))
    ((_ (name arg param ...) body ...)
     (add-method! name 't (lambda (arg param ...) body ...)))))

;; ----- restarts -----
;; Common Lisp style: `restart-case` establishes named ways to continue; a
;; handler running at the raise point (`with-exception-handler`,
;; `handler-bind`) can pick one with `invoke-restart`, which unwinds to the
;; `restart-case` and runs that restart's body.

(define-record-type restart (%make-restart name proc formals) restart?
  (name restart-name)
  (proc %restart-proc)
  ;; The restart's parameter list as written, for debuggers.
  (formals restart-formals))

(define %restarts (make-parameter '()))
(define (compute-restarts) (%restarts))
(define (find-restart name)
  (find (lambda (r) (eq? (restart-name r) name)) (%restarts)))
(define (invoke-restart r . args)
  (let ((r (if (restart? r) r (or (find-restart r) (error "no such restart:" r)))))
    (apply (%restart-proc r) args)))
(define (%with-restarts rs thunk)
  (parameterize ((%restarts (append rs (%restarts)))) (thunk)))

(define-syntax restart-case
  (syntax-rules ()
    ((_ expr (name formals body ...) ...)
     ((call/cc
       (lambda (k)
         (let ((rs (list (%make-restart 'name
                                        (lambda args
                                          (k (lambda () (apply (lambda formals body ...) args))))
                                        'formals)
                         ...)))
           (let ((v (%with-restarts rs (lambda () expr))))
             (lambda () v)))))))))

(define-syntax handler-bind
  (syntax-rules ()
    ((_ ((pred handler) ...) body ...)
     (with-exception-handler
      (lambda (c)
        (cond ((pred c) (handler c)) ...
              (else (raise-continuable c))))
      (lambda () body ...)))))

;; ----- documentation -----

(define-syntax help
  (syntax-rules ()
    ((_ name) (displayln (%describe 'name)))))

;; ----- misc -----

(define (identity x) x)
(define (compose . fs)
  (if (null? fs)
      identity
      (let ((f (car fs)) (g (apply compose (cdr fs))))
        (lambda args (f (apply g args))))))
(define (square x) (* x x))
(define (boolean=? a b . more) (and (eq? a b) (or (null? more) (apply boolean=? b more))))
(define (symbol=? a b . more) (and (eq? a b) (or (null? more) (apply symbol=? b more))))
(define (floor/ n d) (values (floor-quotient n d) (floor-remainder n d)))
(define (truncate/ n d) (values (truncate-quotient n d) (truncate-remainder n d)))
(define (exact-integer-sqrt n)
  (let ((s (%exact-integer-sqrt n))) (values s (- n (* s s)))))
(define (call-with-output-string proc)
  (let ((port (open-output-string))) (proc port) (get-output-string port)))

;; ----- channels -----

(define (make-channel [capacity 0] #:bytes [bytes #f])
  "A channel buffering up to CAPACITY messages (0: a rendezvous, where a send
waits for a receiver) and, with #:bytes, up to BYTES bytes of strings.
The current scope owns it: shutting the scope closes it."
  (scope-own! (%make-channel capacity bytes) channel-close channel-closed?))

(define-syntax %select-op
  (syntax-rules (recv send timeout)
    ((_ (recv ch (v) body ...)) (list 'recv ch (lambda (v) (if #f #f) body ...)))
    ((_ (send ch x body ...)) (list 'send ch x (lambda (_) (if #f #f) body ...)))
    ((_ (timeout ms body ...)) (list 'timeout ms (lambda (_) (if #f #f) body ...)))))

(define (%select-run ops)
  (let ((r (%select ops)))
    ((list-ref (list-ref ops (car r)) (if (eq? (car (list-ref ops (car r))) 'send) 3 2)) (cdr r))))

;; (select (recv ch (v) body ...) (send ch x body ...) (timeout ms body ...))
;; waits for the first clause whose operation can happen, does only that one
;; and runs its body.
(define-syntax select
  (syntax-rules ()
    ((_ clause ...) (%select-run (list (%select-op clause) ...)))))
;; ----- scopes -----

;; A scope owns what code running in it creates or registers: tasks,
;; channels, processes, registry entries, and whatever else is handed to
;; `scope-own!` with a cleanup. Shutting a scope shuts its child scopes,
;; then runs the cleanups, newest first; nothing it owned is left behind.
;; Something meant to outlive its scope (a document, a persistent task)
;; moves to a longer-lived one with `scope-transfer!`.

(define-record-type scope
  (%make-scope name parent children resources serial live? pending)
  scope?
  (name scope-name)
  (parent scope-parent)
  (children %scope-children %set-scope-children!)
  ;; resource -> (serial cleanup . done?)
  (resources %scope-resources)
  (serial %scope-serial %set-scope-serial!)
  (live? scope-live? %set-scope-live!)
  ;; While a package generation loads: the registrations it makes, held
  ;; back (newest first) until it is published; else #f.
  (pending %scope-pending %set-scope-pending!))

;; The scope that owns each resource, without keeping the resource alive.
(define %owners (make-weak-hash-table))

(define %root-scope (%make-scope 'root #f '() (make-hash-table eq?) 0 #t #f))

(define current-scope (make-parameter %root-scope))

(define (make-scope [name #f] #:parent [parent (current-scope)])
  "A new scope, owned by PARENT (the current scope): shutting PARENT shuts it."
  (unless (scope-live? parent) (error "make-scope: the parent scope is shut down" parent))
  (let ((s (%make-scope name parent '() (make-hash-table eq?) 0 #t #f)))
    (%set-scope-children! parent (cons s (%scope-children parent)))
    s))

(define-syntax with-scope
  (syntax-rules ()
    ((_ s body ...) (parameterize ((current-scope s)) body ...))))

(define (%scope-add! s resource cleanup done?)
  (unless (scope-live? s) (error "the scope is shut down" (scope-name s)))
  (let ((table (%scope-resources s)) (n (+ 1 (%scope-serial s))))
    (%set-scope-serial! s n)
    ;; Forget finished resources now and then, so a long-lived scope
    ;; spawning many short tasks does not grow without bound.
    (when (and (> (hash-table-count table) 64) (= 0 (modulo n 64)))
      (for-each (lambda (r) (when ((cddr (hash-table-ref table r)) r) (hash-table-delete! table r)))
                (hash-table-keys table)))
    (hash-table-set! table resource (cons n (cons cleanup done?)))
    (hash-table-set! %owners resource s)
    resource))

(define (scope-own! resource cleanup [done? (lambda (r) #f)])
  "Let the current scope own RESOURCE: shutting the scope calls (CLEANUP
RESOURCE), unless (DONE? RESOURCE) says it has already ended. Returns RESOURCE."
  (%scope-add! (current-scope) resource cleanup done?))

(define (scope-of resource)
  "The scope that owns RESOURCE, or #f."
  (hash-table-ref/default %owners resource #f))

(define (scope-disown! resource)
  "Let no scope own RESOURCE any more; returns RESOURCE."
  (let ((s (scope-of resource)))
    (when s
      (hash-table-delete! (%scope-resources s) resource)
      (hash-table-delete! %owners resource))
    resource))

(define (scope-transfer! resource to)
  "Move RESOURCE, with its cleanup, to the scope TO (for something that must
outlive the scope that made it). Returns RESOURCE."
  (let* ((from (or (scope-of resource) (error "scope-transfer!: no scope owns it" resource)))
         (entry (hash-table-ref (%scope-resources from) resource)))
    (scope-disown! resource)
    (%scope-add! to resource (cadr entry) (cddr entry))))

(define (scope-resources s)
  "What S owns, oldest first."
  (let ((table (%scope-resources s)))
    (map cdr (sort (map (lambda (r) (cons (car (hash-table-ref table r)) r)) (hash-table-keys table))
                   (lambda (a b) (< (car a) (car b)))))))

(define (scope-children s) (%scope-children s))

(define (scope-shutdown! s)
  "Shut S: its child scopes, then its cleanups, newest first. A failing
cleanup does not stop the others; the first failure is raised at the end."
  (when (scope-live? s)
    (let ((failure #f))
      (for-each (lambda (c)
                  (guard (e (#t (unless failure (set! failure e))))
                    (scope-shutdown! c)))
                (%scope-children s))
      (%set-scope-live! s #f)
      (for-each (lambda (r)
                  (let ((entry (hash-table-ref (%scope-resources s) r)))
                    (hash-table-delete! (%scope-resources s) r)
                    (hash-table-delete! %owners r)
                    (unless ((cddr entry) r)
                      (guard (e (#t (unless failure (set! failure e))))
                        ((cadr entry) r)))))
                (reverse (scope-resources s)))
      (%set-scope-children! s '())
      (let ((parent (scope-parent s)))
        (when parent
          (%set-scope-children! parent (remove (lambda (c) (eq? c s)) (%scope-children parent)))))
      (when failure (raise failure)))))

(define (scope-procedure proc)
  "PROC, bound to the current scope: once that scope is shut down, calling it
does nothing and returns #f. For callbacks handed to longer-lived code, so a
late call cannot reach state the scope's replacement now owns."
  (let ((s (current-scope)))
    (lambda args (and (scope-live? s) (apply proc args)))))

(define %spawn spawn)
(define (spawn thunk)
  "Run THUNK in a new task, owned by the current scope: shutting the scope
cancels it. The task runs in that scope too."
  (scope-own! (%spawn thunk) task-cancel task-done?))

;; ----- registries -----

;; A registry maps names to values (commands, keymaps, hooks...). Each
;; entry is owned by the scope that added it; shutting that scope removes
;; the entry, unless another scope has replaced it since.

(define-record-type registry
  (%make-registry name table)
  registry?
  (name registry-name)
  (table %registry-table))

(define-record-type %registration
  (%make-registration registry key value)
  %registration?
  (registry %registration-registry)
  (key %registration-key)
  (value %registration-value))

(define (make-registry [name #f]) (%make-registry name (make-hash-table)))

(define (registry-add! reg key value)
  "Map KEY to VALUE in REG, owned by the current scope; returns VALUE.
While a package generation loads, the entry waits until it is published."
  (let ((s (current-scope)))
    (if (%scope-pending s)
        (begin
          (%set-scope-pending! s (cons (lambda () (%registry-add! reg key value)) (%scope-pending s)))
          value)
        (%registry-add! reg key value))))

(define (%registry-add! reg key value)
  (let ((table (%registry-table reg)))
    (let ((old (hash-table-ref/default table key #f)))
      (when old (scope-disown! old)))
    (let ((r (%make-registration reg key value)))
      (hash-table-set! table key r)
      (scope-own! r (lambda (r)
                      ;; Only if it is still this registration.
                      (when (eq? (hash-table-ref/default table key #f) r)
                        (hash-table-delete! table key))))
      value)))

(define (registry-remove! reg key)
  (let ((r (hash-table-ref/default (%registry-table reg) key #f)))
    (when r
      (scope-disown! r)
      (hash-table-delete! (%registry-table reg) key))))

(define (registry-ref reg key [default #f])
  (let ((r (hash-table-ref/default (%registry-table reg) key #f)))
    (if r (%registration-value r) default)))

(define (registry-keys reg) (hash-table-keys (%registry-table reg)))

(define (registry-owner reg key)
  "The scope that owns KEY's entry in REG, or #f."
  (let ((r (hash-table-ref/default (%registry-table reg) key #f)))
    (and r (scope-of r))))
;; ----- packages -----

;; A package is a file (and the files it requires from its directory)
;; loaded as a generation: fresh modules and a scope of its own. Loading
;; it again loads the next generation beside the current one, its
;; registry entries held back; if that fails, the new generation is shut
;; and the current one is left as it was. If it succeeds, its modules and
;; registry entries are published at once and the previous generation's
;; scope is shut: its tasks are cancelled, its processes killed and the
;; entries the new generation did not replace removed. Code that must
;; outlive a reload moves its task to a longer-lived scope; closures keep
;; the generation they were made in.

(define-record-type package
  (%make-package name path generation scope module)
  package?
  (name package-name)
  (path package-path)
  (generation package-generation)
  (scope package-scope)
  (module package-module))

(define %packages (make-hash-table eq?))

(define (find-package name) (hash-table-ref/default %packages name #f))

(define (packages) (hash-table-values %packages))

(define (load-package name path)
  "Load the package NAME from the file PATH, or its next generation if it is
loaded; returns the generation. On failure nothing visible changes."
  (let* ((old (find-package name))
         (generation (if old (+ 1 (package-generation old)) 1))
         (s (make-scope name #:parent %root-scope)))
    (%set-scope-pending! s '())
    (let ((module (guard (e (#t (%package-discard) (scope-shutdown! s) (raise e)))
                    (with-scope s (%package-stage path generation)))))
      ;; Publish: modules, then the held-back registrations, in order.
      (%package-publish)
      (let ((pending (reverse (%scope-pending s))))
        (%set-scope-pending! s #f)
        (with-scope s (for-each (lambda (add!) (add!)) pending)))
      (hash-table-set! %packages name (%make-package name path generation s module))
      (when old (scope-shutdown! (package-scope old)))
      generation)))

(define (unload-package name)
  "Shut the package NAME: everything its scope owns goes."
  (let ((p (find-package name)))
    (when p
      (hash-table-delete! %packages name)
      (scope-shutdown! (package-scope p)))))
