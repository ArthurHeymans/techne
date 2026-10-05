;;; Core library written in Scheme. Higher-order procedures live here rather
;;; than in Rust so that they call closures through the ordinary VM path
;;; (proper tail calls, resumable later). Derived syntax is defined with
;;; syntax-rules.

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
         (%case-lambda-dispatch args clause ...)))))

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
         (lambda vals (%set-each! vals var ...)))))))

(define-syntax %set-each!
  (syntax-rules ()
    ((_ vals) (void))
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

(define-syntax make-promise
  (syntax-rules ()
    ((_ v) (%make-promise #t v))))

(define-record-type promise (%make-promise done? value) promise?
  (done? %promise-done? %set-promise-done!)
  (value %promise-value %set-promise-value!))

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

(define (make-parameter init . converter)
  (let* ((convert (if (null? converter) (lambda (x) x) (car converter)))
         (value (convert init)))
    (lambda args
      (cond ((null? args) value)
            ((eq? (car args) '%parameter-set!) (set! value (cadr args)))
            ((eq? (car args) '%parameter-convert) (convert (cadr args)))
            (else (error "parameter: unexpected arguments" args))))))

(define-syntax parameterize
  (syntax-rules ()
    ((_ ((param value) ...) body ...)
     (let ((params (list param ...))
           (new (list (param '%parameter-convert value) ...)))
       (let ((old (map (lambda (p) (p)) params)))
         (dynamic-wind
           (lambda () (for-each (lambda (p v) (p '%parameter-set! v)) params new))
           (lambda () body ...)
           (lambda () (for-each (lambda (p v) (p '%parameter-set! v)) params old))))))))

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

(define (cadr x) (car (cdr x)))
(define (cddr x) (cdr (cdr x)))
(define (caar x) (car (car x)))
(define (cdar x) (cdr (car x)))
(define (caddr x) (car (cdr (cdr x))))
(define (cdddr x) (cdr (cdr (cdr x))))
(define (cadddr x) (car (cdr (cdr (cdr x)))))
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
(define (list-copy l) (map (lambda (x) x) l))
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

(define (vector-map f v) (list->vector (map f (vector->list v))))
(define (vector-for-each f v) (for-each f (vector->list v)))
(define (vector-append . vs) (list->vector (apply append (map vector->list vs))))
(define (string-map f s) (list->string (map f (string->list s))))
(define (string-for-each f s) (for-each f (string->list s)))
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

(define-record-type restart (%make-restart name proc) restart?
  (name restart-name)
  (proc %restart-proc))

(define %restarts '())
(define (compute-restarts) %restarts)
(define (find-restart name)
  (find (lambda (r) (eq? (restart-name r) name)) %restarts))
(define (invoke-restart r . args)
  (let ((r (if (restart? r) r (or (find-restart r) (error "no such restart:" r)))))
    (apply (%restart-proc r) args)))
(define (%with-restarts rs thunk)
  (let ((old %restarts))
    (dynamic-wind (lambda () (set! %restarts (append rs old)))
                  thunk
                  (lambda () (set! %restarts old)))))

(define-syntax restart-case
  (syntax-rules ()
    ((_ expr (name formals body ...) ...)
     ((call/cc
       (lambda (k)
         (let ((rs (list (%make-restart 'name
                                        (lambda args
                                          (k (lambda () (apply (lambda formals body ...) args)))))
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

;; ----- misc -----

(define (identity x) x)
(define (compose . fs)
  (if (null? fs)
      identity
      (let ((f (car fs)) (g (apply compose (cdr fs))))
        (lambda args (f (apply g args))))))
(define (square x) (* x x))
(define (boolean=? a b) (eq? a b))
(define (symbol=? a b) (eq? a b))
(define (call-with-output-string proc)
  (let ((port (open-output-string))) (proc port) (get-output-string port)))
