;;; Core library written in Scheme. Higher-order procedures live here rather
;;; than in Rust so that they call closures through the ordinary VM path
;;; (proper tail calls, resumable later). Derived syntax is defined with
;;; syntax-rules.

;; ----- control (no native callbacks, so tasks can suspend inside) -----

(define (call/cc f)
  "Call F with the current continuation, an escape-only one.
Calling it returns its arguments from `call/cc` again, while that call
has not returned; continuations cannot be re-entered."
  (%with-escape f))
(define call-with-current-continuation call/cc
  "Call F with the current continuation; the long name of `call/cc`.")
(define call/ec call/cc
  "Call F with an escape continuation, as `call/cc` does.")

(define (dynamic-wind before thunk after)
  "Call THUNK, calling BEFORE first and AFTER once it is left.
AFTER runs however THUNK is left: by returning, an error or an escape."
  (before)
  (%push-wind after)
  (let ((result (thunk)))
    (%pop-handler)
    (after)
    result))

(define (with-exception-handler handler thunk)
  "Call THUNK with HANDLER handling what it raises.
HANDLER is called with the condition where it was raised; for `raise`
it must not return."
  (%push-handler handler)
  (let ((result (thunk)))
    (%pop-handler)
    result))

(define (call-with-values producer consumer)
  "Call CONSUMER with the values PRODUCER returns."
  (let ((v (producer)))
    (if (%values? v) (apply consumer (%values->list v)) (consumer v))))

;; ----- syntax -----

(define-syntax do
  (syntax-rules ()
    "Loop: bind each VAR to INIT, then to STEP, until TEST holds.
(do ((var init step ...) ...) (test expr ...) command ...) runs the
COMMANDs each time round and returns the value of the last EXPR."
    ((_ ((var init step ...) ...) (test expr ...) command ...)
     (let loop ((var init) ...)
       (if test
           (begin (void) expr ...)
           (begin command ... (loop (do "step" var step ...) ...)))))
    ((_ "step" x) x)
    ((_ "step" x y) y)))

(define-syntax case-lambda
  (syntax-rules ()
    "Make a procedure choosing its body by the number of arguments.
(case-lambda (formals body ...) ...) runs the first clause whose
FORMALS take the arguments given."
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
    "Bind FORMALS to the values of EXPR and evaluate BODY.
(receive formals expr body ...), as SRFI 8 has it."
    ((_ formals expr body ...)
     (call-with-values (lambda () expr) (lambda formals body ...)))))

(define-syntax let-values
  (syntax-rules ()
    "Bind each FORMALS to the values of its EXPR, then evaluate BODY.
(let-values ((formals expr) ...) body ...)."
    ((_ () body ...) (let () body ...))
    ((_ ((formals expr) rest ...) body ...)
     (call-with-values (lambda () expr)
       (lambda formals (let-values (rest ...) body ...))))))

(define-syntax let*-values
  (syntax-rules ()
    "Bind each FORMALS to the values of its EXPR in turn, then BODY.
Each EXPR sees the bindings before it."
    ((_ bindings body ...) (let-values bindings body ...))))

(define-syntax define-values
  (syntax-rules ()
    "Define each VAR as the values of EXPR.
(define-values (var ...) expr); a rest variable takes the values left."
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
    "Raise an error naming EXPR unless its value is true."
    ((_ expr) (unless expr (error "assertion failed:" 'expr)))))

(define-syntax delay
  (syntax-rules ()
    "Return a promise to evaluate EXPR when `force` asks, once."
    ((_ expr) (%make-promise #f (lambda () expr)))))

(define-syntax delay-force
  (syntax-rules ()
    "Return a promise to evaluate EXPR, itself a promise, when forced.
Forcing it forces what EXPR gives, in constant space."
    ((_ expr) (%make-promise #f (lambda () expr)))))

(define-record-type promise (%make-promise done? value) promise?
  (done? %promise-done? %set-promise-done!)
  (value %promise-value %set-promise-value!))

(define (make-promise v)
  "Return a promise whose value is V, or V itself if it is a promise."
  (if (promise? v) v (%make-promise #t v)))

(define (force p)
  "Return the value of the promise P, evaluating it the first time.
Anything other than a promise is returned as it is."
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
  "Return a new parameter whose value is INIT.
A parameter is called with no arguments for its value; `parameterize`
changes it for the extent of its body, in the current task only. The
first of CONVERTER, if given, converts each value it is given."
  (%make-parameter-with-key (%fresh-key) init (if (null? converter) (lambda (x) x) (car converter))))

(define (%parameter-key p) (%record-ref p %parameter-type 1))
(define (%parameter-convert p v) ((%record-ref p %parameter-type 2) v))

(define-syntax parameterize
  (syntax-rules ()
    "Evaluate BODY with each PARAM giving the value of its VALUE.
(parameterize ((param value) ...) body ...); the values are the
current task's only, and are restored however BODY is left."
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
(define current-output-port (%make-parameter-with-key (%output-port-key) (%stdout) (lambda (x) x))
  "Return the port output goes to by default, a parameter.")
(define %stdin (%make-stdin))
(define current-input-port (%make-parameter-with-key (%input-port-key) %stdin (lambda (x) x))
  "Return the port input comes from by default, a parameter.")
(define current-error-port (make-parameter (%stderr))
  "Return the port errors are written to, a parameter.")

(define (get-environment-variables)
  "Return the environment variables as a list of (name . value)."
  (let loop ((l (%environment-variables)) (acc '()))
    (if (null? l) (reverse acc) (loop (cddr l) (cons (cons (car l) (cadr l)) acc)))))

(define (environment . sets)
  "Return a new module seeing only the libraries SETS import.
Each of SETS is an import set, as `import` takes them."
  (%environment sets))
(define (scheme-report-environment . version)
  "Return a new module seeing only (scheme r5rs).
VERSION is ignored."
  (%environment '((scheme r5rs))))
(define (null-environment . version)
  "Return a new module seeing nothing.
VERSION is ignored."
  (%environment '()))
(define (interaction-environment)
  "Return the name of the current module, where the REPL evaluates."
  (current-module))

(define (call-with-port port proc)
  "Call PROC with PORT, then close PORT; return what PROC returns."
  (call-with-values (lambda () (proc port))
    (lambda vals (close-port port) (apply values vals))))
(define (call-with-input-file file proc)
  "Call PROC with a port reading FILE, closed after."
  (call-with-port (open-input-file file) proc))
(define (call-with-output-file file proc)
  "Call PROC with a port writing FILE, closed after."
  (call-with-port (open-output-file file) proc))
(define (with-input-from-file file thunk)
  "Call THUNK with the current input port reading FILE."
  (call-with-input-file file (lambda (port) (parameterize ((current-input-port port)) (thunk)))))
(define (with-output-to-file file thunk)
  "Call THUNK with the current output port writing FILE."
  (call-with-output-file file (lambda (port) (parameterize ((current-output-port port)) (thunk)))))

(define (with-output-to-string thunk)
  "Call THUNK and return what it writes to the current output port."
  (let ((port (open-output-string)))
    (parameterize ((current-output-port port)) (thunk))
    (get-output-string port)))

;; ----- match helpers -----

(define (%list-all? p l)
  (cond ((null? l) #t) ((pair? l) (and (p (car l)) (%list-all? p (cdr l)))) (else #f)))

(define-syntax match-lambda
  (syntax-rules ()
    "Make a procedure of one argument matching it as `match` does.
(match-lambda clause ...) is (lambda (x) (match x clause ...))."
    ((_ clause ...) (lambda (x) (match x clause ...)))))

(define-syntax match-let
  (syntax-rules ()
    "Bind the variables of each PAT matching the value of its E.
(match-let ((pat e) ...) body ...) then evaluates BODY."
    ((_ ((pat e) ...) body ...) (match (list e ...) ((pat ...) body ...)))))

;; ----- lists -----

(define (caar x)
  "Return (car (car X))."
  (car (car x)))
(define (cadr x)
  "Return (car (cdr X))."
  (car (cdr x)))
(define (cdar x)
  "Return (cdr (car X))."
  (cdr (car x)))
(define (cddr x)
  "Return (cdr (cdr X))."
  (cdr (cdr x)))
(define (caddr x)
  "Return (car (cdr (cdr X)))."
  (car (cdr (cdr x))))
(define (cdddr x)
  "Return (cdr (cdr (cdr X)))."
  (cdr (cdr (cdr x))))
(define (first x)
  "Return the first element of the list X."
  (car x))
(define (second x)
  "Return the second element of the list X."
  (cadr x))
(define (third x)
  "Return the third element of the list X."
  (caddr x))
(define (rest x)
  "Return the list X without its first element."
  (cdr x))

(define (%map1 f l)
  (let loop ((l l) (acc '()))
    (if (null? l) (reverse acc) (loop (cdr l) (cons (f (car l)) acc)))))

(define (%any-null? ls)
  (cond ((null? ls) #f) ((null? (car ls)) #t) (else (%any-null? (cdr ls)))))

(define (map f l . more)
  "Return the results of calling F on the elements of L in order.
With MORE lists, F takes an element of each; the shortest list ends
it."
  (if (null? more)
      (%map1 f l)
      (let loop ((ls (cons l more)) (acc '()))
        (if (%any-null? ls)
            (reverse acc)
            (loop (%map1 cdr ls) (cons (apply f (%map1 car ls)) acc))))))

(define (for-each f l . more)
  "Call F on each element of L in order, for its effect.
With MORE lists, F takes an element of each; the shortest list ends
it."
  (if (null? more)
      (let loop ((l l)) (when (pair? l) (f (car l)) (loop (cdr l))))
      (let loop ((ls (cons l more)))
        (unless (%any-null? ls)
          (apply f (%map1 car ls))
          (loop (%map1 cdr ls))))))

(define (filter p l)
  "Return the elements of L for which P holds, in order."
  (let loop ((l l) (acc '()))
    (cond ((null? l) (reverse acc))
          ((p (car l)) (loop (cdr l) (cons (car l) acc)))
          (else (loop (cdr l) acc)))))

(define (remove p l)
  "Return the elements of L for which P does not hold, in order."
  (filter (lambda (x) (not (p x))) l))

(define (partition p l)
  "Return two values: the elements of L for which P holds, and the rest."
  (let loop ((l l) (yes '()) (no '()))
    (cond ((null? l) (values (reverse yes) (reverse no)))
          ((p (car l)) (loop (cdr l) (cons (car l) yes) no))
          (else (loop (cdr l) yes (cons (car l) no))))))

(define (fold-left f acc l)
  "Fold L from the left: (F (F ACC e1) e2) and so on."
  (let loop ((acc acc) (l l)) (if (null? l) acc (loop (f acc (car l)) (cdr l)))))
(define (fold-right f acc l)
  "Fold L from the right: (F e1 (F e2 ... ACC))."
  (let loop ((l (reverse l)) (acc acc)) (if (null? l) acc (loop (cdr l) (f (car l) acc)))))
(define (fold kons knil l)
  "Fold L from the left with KONS, starting from KNIL, as SRFI 1 does.
KONS is called as (KONS element accumulated)."
  (let loop ((acc knil) (l l)) (if (null? l) acc (loop (kons (car l) acc) (cdr l)))))
(define (foldl f acc l)
  "Fold L from the left with F, starting from ACC, as Racket's foldl.
F is called as (F element accumulated)."
  (fold f acc l))
(define (reduce f ridentity l)
  "Fold L with F from its first element, or return RIDENTITY if empty."
  (if (null? l) ridentity (fold f (car l) (cdr l))))

(define (append-map f l)
  "Return the lists F gives for the elements of L, appended."
  (apply append (map f l)))
(define (filter-map f l)
  "Return the true results of calling F on the elements of L."
  (let loop ((l l) (acc '()))
    (if (null? l) (reverse acc)
        (let ((v (f (car l)))) (loop (cdr l) (if v (cons v acc) acc))))))
(define (find p l)
  "Return the first element of L for which P holds, or #f."
  (cond ((null? l) #f) ((p (car l)) (car l)) (else (find p (cdr l)))))
(define (find-tail p l)
  "Return the first tail of L whose car P holds for, or #f."
  (cond ((null? l) #f) ((p (car l)) l) (else (find-tail p (cdr l)))))
(define (any p l)
  "Return the first true value of P on the elements of L, or #f."
  (and (pair? l) (or (p (car l)) (any p (cdr l)))))
(define (every p l)
  "Return #f if P is false for an element of L, else its last value.
For an empty L, return #t."
  (let loop ((l l) (last #t))
    (if (null? l) last (let ((v (p (car l)))) (and v (loop (cdr l) v))))))
(define (count p l)
  "Return how many elements of L P holds for."
  (let loop ((l l) (n 0)) (if (null? l) n (loop (cdr l) (if (p (car l)) (+ n 1) n)))))
(define (delete x l)
  "Return L without the elements `equal?` to X."
  (remove (lambda (y) (equal? x y)) l))
(define (delete-duplicates l)
  "Return L without its later duplicates, by `equal?`."
  (let loop ((l l) (acc '()))
    (cond ((null? l) (reverse acc))
          ((member (car l) acc) (loop (cdr l) acc))
          (else (loop (cdr l) (cons (car l) acc))))))
(define (last-pair l)
  "Return the last pair of the list L."
  (if (pair? (cdr l)) (last-pair (cdr l)) l))
(define (last l)
  "Return the last element of the list L."
  (car (last-pair l)))
(define (list-index p l)
  "Return the index of the first element of L P holds for, or #f."
  (let loop ((l l) (i 0)) (cond ((null? l) #f) ((p (car l)) i) (else (loop (cdr l) (+ i 1))))))
(define (take l n)
  "Return the first N elements of L."
  (if (= n 0) '() (cons (car l) (take (cdr l) (- n 1)))))
(define (drop l n)
  "Return L without its first N elements."
  (if (= n 0) l (drop (cdr l) (- n 1))))
(define (list-copy l)
  "Return a copy of the pairs of the list L."
  (if (pair? l)
      (let ((head (cons (car l) '())))
        (let loop ((tail head) (l (cdr l)))
          (if (pair? l)
              (let ((next (cons (car l) '())))
                (set-cdr! tail next)
                (loop next (cdr l)))
              (begin (set-cdr! tail l) head))))
      l))
(define (list-set! l k v)
  "Store V as element K of the list L."
  (set-car! (list-tail l k) v))
(define (make-list n . fill)
  "Return a list of N elements, each the first of FILL or #f."
  (let ((x (if (null? fill) #f (car fill))))
    (let loop ((i 0) (acc '())) (if (= i n) acc (loop (+ i 1) (cons x acc))))))
(define (list-tabulate n f)
  "Return the list of (F I) for each I from 0 below N."
  (let loop ((i (- n 1)) (acc '())) (if (< i 0) acc (loop (- i 1) (cons (f i) acc)))))
(define (iota n . args)
  "Return N numbers from START by STEP, from ARGS: 0 and 1 by default.
ARGS is empty, (START) or (START STEP)."
  (let ((start (if (pair? args) (car args) 0))
        (step (if (and (pair? args) (pair? (cdr args))) (cadr args) 1)))
    (let loop ((i (- n 1)) (acc '()))
      (if (< i 0) acc (loop (- i 1) (cons (+ start (* i step)) acc))))))
(define (range . args)
  "Return the integers from START up to END, ARGS being (END) or both.
With ARGS (END), START is 0."
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
  "Return a sorted list or vector of A by the order B, stably.
It takes (sort sequence less?) or (sort less? sequence): A and B are
the sequence and the procedure LESS? in either order."
  (cond ((procedure? a) (sort b a))
        ((vector? a) (list->vector (%sort (vector->list a) b)))
        (else (%sort a b))))
(define list-sort sort
  "Return the list or vector A sorted by B, or B sorted by A.
The R7RS-large name of `sort`.")

;; ----- vectors, strings -----

(define (vector-map f v . vs)
  "Return a vector of the results of F on the elements of V.
With VS, F takes an element of each vector."
  (list->vector (apply map f (vector->list v) (map vector->list vs))))
(define (vector-for-each f v . vs)
  "Call F on each element of V in order.
With VS, F takes an element of each vector."
  (apply for-each f (vector->list v) (map vector->list vs)))
(define (vector-append . vs)
  "Return a new vector of the elements of VS in order."
  (list->vector (apply append (map vector->list vs))))
(define (string-map f s . ss)
  "Return a string of the characters F gives for those of S.
With SS, F takes a character of each string."
  (list->string (apply map f (string->list s) (map string->list ss))))
(define (string-for-each f s . ss)
  "Call F on each character of S in order.
With SS, F takes a character of each string."
  (apply for-each f (string->list s) (map string->list ss)))
(define (string-null? s)
  "Return #t if S is the empty string."
  (= (string-length s) 0))

;; ----- hash tables -----

(define %absent (list 'absent))
(define (hash-table-ref h k . o)
  "Return the value of K in H.
When H has no K, call the first of O, a procedure of no arguments, and
return its result; without it a missing key is an error. With a second
procedure in O, return the result of calling it on K's value instead."
  (let ((v (hash-table-ref/default h k %absent)))
    (cond ((not (eq? v %absent)) (if (and (pair? o) (pair? (cdr o))) ((cadr o) v) v))
          ((pair? o) ((car o)))
          (else (error "hash-table-ref: key not found:" k)))))
(define (hash-table-update! h k f . o)
  "Store (F value) as the value of K in H.
The value is K's, found as `hash-table-ref` finds it with O."
  (hash-table-set! h k (f (apply hash-table-ref h k o))))
(define (hash-table-update!/default h k f d)
  "Store (F value) as the value of K in H, the value K's or else D."
  (hash-table-set! h k (f (hash-table-ref/default h k d))))
(define (hash-table-for-each h f)
  "Call F with the key and value of each entry of H."
  (for-each (lambda (kv) (f (car kv) (cdr kv))) (hash-table->alist h)))
(define hash-table-walk hash-table-for-each
  "Call F with the key and value of each entry of H.
The old name of `hash-table-for-each`.")
(define (hash-table-fold h f init)
  "Return the result of folding F over the entries of H from INIT.
F takes a key, its value and the result so far. The arguments may also
come as F INIT H."
  (if (hash-table? h)
      (fold (lambda (kv acc) (f (car kv) (cdr kv) acc)) init (hash-table->alist h))
      (hash-table-fold init h f)))
(define hash-table-size hash-table-count
  "Return the number of entries of TABLE, as `hash-table-count` does.")
(define hash-table-exists? hash-table-contains?
  "Return #t if TABLE has an entry for KEY, as `hash-table-contains?` does.")
(define (hash-table-merge! to from)
  "Add the entries of FROM to TO and return TO.
Entries of FROM replace those of the same keys in TO."
  (hash-table-for-each from (lambda (k v) (hash-table-set! to k v)))
  to)
(define (alist->hash-table alist . o)
  "Return a new hash table of the entries of ALIST.
ALIST is a list of (key . value), whose first entry for a key wins. O
is the equivalence and hash procedure as `make-hash-table` takes them."
  (let ((h (apply make-hash-table o)))
    (for-each (lambda (kv) (unless (hash-table-contains? h (car kv)) (hash-table-set! h (car kv) (cdr kv))))
              alist)
    h))

;; ----- generic functions -----
;; Single dispatch on the first argument's type, along the chain
;; record type -> record -> t, integer/float -> number -> t, pair/null -> list -> t,
;; named foreign type -> foreign -> t. A generic is an applicable record.

(define %generic-type (%make-rtd 'generic '(proc name methods) 0))

(define (%find-method methods x)
  (let loop ((key (%type-key x)))
    (and key (or (hash-table-ref/default methods key #f) (loop (%type-parent key))))))

(define (make-generic name)
  "Return a new generic function called NAME, without methods.
It calls the method for its first argument's type, the nearest along
record type, `record`, `t`; integer or float, `number`, `t`; pair or
null, `list`, `t`; a named foreign type, `foreign`, `t`."
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

(define (generic? x)
  "Return #t if X is a generic function."
  (%record? x %generic-type))
(define (generic-name g)
  "Return the name of the generic function G."
  (%record-ref g %generic-type 1))
(define (add-method! g type proc)
  "Make PROC the method of the generic function G for TYPE."
  (hash-table-set! (%record-ref g %generic-type 2) type proc))
(define (find-method g x)
  "Return the method of the generic function G for X, or #f."
  (%find-method (%record-ref g %generic-type 2) x))
(define (applicable? g x)
  "Return #t if the generic function G has a method for X."
  (if (find-method g x) #t #f))

(define-syntax define-generic
  (syntax-rules ()
    "Define NAME as a generic function, without methods.
(define-generic (name arg ...)) or (define-generic name)."
    ((_ (name arg ...)) (define name (make-generic 'name)))
    ((_ name) (define name (make-generic 'name)))))

(define-syntax define-method
  (syntax-rules ()
    "Add a method to the generic function NAME.
(define-method (name (arg type) param ...) body ...) is for the type
TYPE of ARG; without a type it is the default, for `t`."
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
(define (compute-restarts)
  "Return the restarts available here, innermost first."
  (%restarts))
(define (find-restart name)
  "Return the restart called NAME available here, or #f."
  (find (lambda (r) (eq? (restart-name r) name)) (%restarts)))
(define (invoke-restart r . args)
  "Continue with the restart R, a restart or its name, given ARGS."
  (let ((r (if (restart? r) r (or (find-restart r) (error "no such restart:" r)))))
    (apply (%restart-proc r) args)))
(define (%with-restarts rs thunk)
  (parameterize ((%restarts (append rs (%restarts)))) (thunk)))

(define-syntax restart-case
  (syntax-rules ()
    "Evaluate EXPR with restarts NAME available to its handlers.
(restart-case expr (name formals body ...) ...): a handler invoking
NAME unwinds to here and evaluates BODY with FORMALS bound."
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
    "Evaluate BODY with HANDLERs for the conditions their PRED accepts.
(handler-bind ((pred handler) ...) body ...): a handler runs where the
condition was raised, so it can invoke a restart there."
    ((_ ((pred handler) ...) body ...)
     (with-exception-handler
      (lambda (c)
        (cond ((pred c) (handler c)) ...
              (else (raise-continuable c))))
      (lambda () body ...)))))

;; ----- documentation -----

(define-syntax help
  (syntax-rules ()
    "Print the signature, location and documentation of NAME."
    ((_ name) (displayln (%describe 'name)))))

;; ----- misc -----

(define (identity x)
  "Return X."
  x)
(define (compose . fs)
  "Return the composition of FS: the last is called first."
  (if (null? fs)
      identity
      (let ((f (car fs)) (g (apply compose (cdr fs))))
        (lambda args (f (apply g args))))))
(define (square x)
  "Return X times X."
  (* x x))
(define (boolean=? a b . more)
  "Return #t if the booleans A, B and MORE are all the same."
  (and (eq? a b) (or (null? more) (apply boolean=? b more))))
(define (symbol=? a b . more)
  "Return #t if the symbols A, B and MORE are all the same."
  (and (eq? a b) (or (null? more) (apply symbol=? b more))))
(define (floor/ n d)
  "Return two values: N divided by D rounded down, and the remainder."
  (values (floor-quotient n d) (floor-remainder n d)))
(define (truncate/ n d)
  "Return two values: N divided by D toward zero, and the remainder."
  (values (truncate-quotient n d) (truncate-remainder n d)))
(define (exact-integer-sqrt n)
  "Return two values: the integer square root of N and the remainder."
  (let ((s (%exact-integer-sqrt n))) (values s (- n (* s s)))))
(define (call-with-output-string proc)
  "Call PROC with a string port and return what it wrote there."
  (let ((port (open-output-string))) (proc port) (get-output-string port)))

;; ----- channels -----

(define (make-channel [capacity 0] #:bytes [bytes #f])
  "Return a new channel buffering up to CAPACITY messages.
With CAPACITY 0, a send waits for a receiver; with BYTES, the strings
buffered are at most BYTES bytes. The current scope owns the channel:
shutting the scope closes it."
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
    "Wait for the first of the CLAUSEs that can happen, and do it.
A clause is (recv ch (v) body ...), (send ch x body ...) or (timeout
ms body ...); only that one's operation happens, then its body runs."
    ((_ clause ...) (%select-run (list (%select-op clause) ...)))))
;; ----- scopes -----

;; A scope owns what code running in it creates or registers: tasks,
;; channels, processes, registry entries, and whatever else is handed to
;; `scope-own!` with a cleanup. Shutting a scope shuts its child scopes,
;; then runs the cleanups, newest first; nothing it owned is left behind.
;; Something meant to outlive its scope (a document, a persistent task)
;; moves to a longer-lived one with `scope-transfer!`.

(define-record-type scope
  (%make-scope name parent children resources serial live? pending predecessor)
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
  (pending %scope-pending %set-scope-pending!)
  ;; The scope this one succeeds (a package's previous generation) while
  ;; that one still lives: its registrations are this one's to replace.
  (predecessor %scope-predecessor %set-scope-predecessor!))

;; The scope that owns each resource, without keeping the resource alive.
(define %owners (make-weak-hash-table))

(define %root-scope (%make-scope 'root #f '() (make-hash-table eq?) 0 #t #f #f))

(define current-scope (make-parameter %root-scope)
  "Return the scope owning what is made now, a parameter.")

(define (make-scope [name #f] #:parent [parent (current-scope)])
  "Return a new scope called NAME, owned by PARENT.
PARENT is the current scope by default; shutting it shuts the new one."
  (unless (scope-live? parent) (error "make-scope: the parent scope is shut down" parent))
  (let ((s (%make-scope name parent '() (make-hash-table eq?) 0 #t #f #f)))
    (%set-scope-children! parent (cons s (%scope-children parent)))
    s))

;; As (parameterize ((current-scope s)) body ...), cheaper to expand: it is
;; used in every procedure handed over (scope-procedure).
(define (%call-in-scope s thunk)
  (let ((key (%parameter-key current-scope)) (old (current-scope)))
    (dynamic-wind (lambda () (%task-local-set! key s)) thunk (lambda () (%task-local-set! key old)))))

(define-syntax with-scope
  (syntax-rules ()
    "Evaluate BODY with S the current scope."
    ((_ s body ...) (%call-in-scope s (lambda () body ...)))))

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
  "Let the current scope own RESOURCE, and return RESOURCE.
Shutting the scope calls (CLEANUP RESOURCE), unless (DONE? RESOURCE)
says it has ended already."
  (%scope-add! (current-scope) resource cleanup done?))

(define (scope-of resource)
  "Return the scope that owns RESOURCE, or #f."
  (hash-table-ref/default %owners resource #f))

(define (scope-disown! resource)
  "Let no scope own RESOURCE any more, and return RESOURCE."
  (let ((s (scope-of resource)))
    (when s
      (hash-table-delete! (%scope-resources s) resource)
      (hash-table-delete! %owners resource))
    resource))

(define (scope-transfer! resource to)
  "Move RESOURCE, with its cleanup, to the scope TO; return RESOURCE.
This is for something that must outlive the scope that made it."
  (let* ((from (or (scope-of resource) (error "scope-transfer!: no scope owns it" resource)))
         (entry (hash-table-ref (%scope-resources from) resource)))
    (scope-disown! resource)
    (%scope-add! to resource (cadr entry) (cddr entry))))

(define (scope-resources s)
  "Return what the scope S owns, oldest first."
  (let ((table (%scope-resources s)))
    (map cdr (sort (map (lambda (r) (cons (car (hash-table-ref table r)) r)) (hash-table-keys table))
                   (lambda (a b) (< (car a) (car b)))))))

(define (scope-children s)
  "Return the scopes S owns."
  (%scope-children s))

(define (scope-shutdown! s)
  "Shut the scope S: its child scopes, then its cleanups, newest first.
A failing cleanup does not stop the others; the first failure is raised
at the end."
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
  "Return PROC bound to the current scope.
It runs in that scope, so what it starts (tasks, processes,
registrations) is owned there; once the scope is shut down, calling it
does nothing and returns #f. This is for procedures handed to
longer-lived code (a command to the editor, a callback to a service):
work they start belongs to whoever handed them over, not to whoever
calls them, and a late call cannot reach state the scope's replacement
owns now."
  (let ((s (current-scope)))
    (lambda args (and (scope-live? s) (%call-in-scope s (lambda () (apply proc args)))))))

(define %spawn spawn)
(define (spawn thunk)
  "Run THUNK in a new task owned by the current scope; return the task.
Shutting the scope cancels it. The task runs in that scope too."
  (scope-own! (%spawn thunk) task-cancel task-done?))

;; ----- registries -----

;; A registry maps names to values (commands, keymaps, hooks...). Each
;; entry is owned by the scope that added it. Entries of a key stack: the
;; newest is in effect, and shutting the scope that added it uncovers the
;; one it shadowed, so unloading a package that overrides a command brings
;; the command back. A scope keeps one entry per key: adding again replaces
;; its own. A package's next generation takes the places of the previous
;; one's entries, so a reload never comes out above an override made since.

;; A registration is a fresh pair (value), owned by the scope that made it.
(define-record-type registry
  (%make-registry name table changed)
  registry?
  (name registry-name)
  ;; key -> its registrations, the one in effect first
  (table %registry-table)
  ;; (key value) when the entry in effect for a key changes, VALUE #f when
  ;; none is left; or #f.
  (changed %registry-changed))

(define (make-registry [name #f] #:changed [changed #f])
  "Return a new registry called NAME, mapping keys to values.
Each entry is owned by the scope that added it. (CHANGED key value), if
given, is called when the entry in effect for a key changes, with #f
when none is left."
  (%make-registry name (make-hash-table) changed))

(define (registry-add! reg key value)
  "Map KEY to VALUE in REG, owned by the current scope; return VALUE.
While a package generation loads, the entry waits until it is published."
  (%registry-staged (lambda () (%registry-add! reg key value)))
  value)

(define (registry-remove! reg key)
  "Take back KEY's entry in REG made by the current scope.
Without one, take back the entry in effect; an entry it shadowed is in
effect again. While a package generation loads, this waits until it is
published."
  (%registry-staged
   (lambda ()
     (let* ((stack (hash-table-ref/default (%registry-table reg) key '()))
            (r (or (find (lambda (r) (eq? (scope-of r) (current-scope))) stack) (and (pair? stack) (car stack)))))
       (when r (%registry-swap! reg key r #f #t))))))

;; Do CHANGE now, or when the package generation loading is published.
(define (%registry-staged change)
  (let ((s (current-scope)))
    (if (%scope-pending s) (%set-scope-pending! s (cons change (%scope-pending s))) (change))))

;; The current scope's entry replaces its own or its predecessor's: in
;; place while a generation succeeds another (it keeps the place its
;; predecessor had, below overrides made since), else on top.
(define (%registry-add! reg key value)
  (let* ((s (current-scope)) (p (%scope-predecessor s))
         (stack (hash-table-ref/default (%registry-table reg) key '()))
         (old (find (lambda (r) (let ((o (scope-of r))) (or (eq? o s) (and p (eq? o p))))) stack)))
    (%registry-swap! reg key old (scope-own! (list value) (lambda (r) (%registry-swap! reg key r #f #t))) p)))

;; Replace registration OLD (or #f) of KEY by NEW (#f: by nothing), in
;; OLD's place when IN-PLACE, else NEW on top; tell the registry when the
;; entry in effect changed.
(define (%registry-swap! reg key old new in-place)
  (let* ((table (%registry-table reg))
         (before (hash-table-ref/default table key '()))
         (stack (if (and old in-place)
                    (filter-map (lambda (x) (if (eq? x old) new x)) before)
                    (let ((rest (remove (lambda (x) (eq? x old)) before))) (if new (cons new rest) rest))))
         (changed (%registry-changed reg)))
    (when old (scope-disown! old))
    (if (null? stack) (hash-table-delete! table key) (hash-table-set! table key stack))
    (when (and changed (not (and (pair? before) (pair? stack) (eq? (car before) (car stack)))))
      (changed key (and (pair? stack) (car (car stack)))))))

(define (registry-ref reg key [default #f])
  "Return the value of KEY's entry in effect in REG, else DEFAULT."
  (let ((stack (hash-table-ref/default (%registry-table reg) key '())))
    (if (pair? stack) (car (car stack)) default)))

(define (registry-keys reg)
  "Return the keys of REG."
  (hash-table-keys (%registry-table reg)))

(define (registry-owner reg key)
  "The scope that owns KEY's entry in effect in REG, or #f."
  (let ((stack (hash-table-ref/default (%registry-table reg) key '())))
    (and (pair? stack) (scope-of (car stack)))))

(define (registry-entries reg key)
  "Return KEY's entries in REG, the one in effect first.
Each is (value . scope): what an override shadows, for explaining where
a setting comes from."
  (map (lambda (r) (cons (car r) (scope-of r))) (hash-table-ref/default (%registry-table reg) key '())))

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

(define (find-package name)
  "Return the package called NAME, or #f."
  (hash-table-ref/default %packages name #f))

(define (packages)
  "Return the packages loaded."
  (hash-table-values %packages))

(define (load-package name path)
  "Load the package NAME from the file PATH; return its generation.
If NAME is loaded, this loads its next generation. On failure, nothing
visible changes."
  (let* ((old (find-package name))
         (generation (if old (+ 1 (package-generation old)) 1))
         (s (make-scope name #:parent %root-scope)))
    (%set-scope-pending! s '())
    (when old (%set-scope-predecessor! s (package-scope old)))
    (let ((module (guard (e (#t (%package-discard) (scope-shutdown! s) (raise e)))
                    (with-scope s (%package-stage path generation)))))
      ;; Publish: modules, then the held-back registrations, in order.
      (%package-publish)
      (let ((pending (reverse (%scope-pending s))))
        (%set-scope-pending! s #f)
        (with-scope s (for-each (lambda (add!) (add!)) pending)))
      (hash-table-set! %packages name (%make-package name path generation s module))
      (when old (scope-shutdown! (package-scope old)))
      (%set-scope-predecessor! s #f)
      generation)))

(define (unload-package name)
  "Shut the package NAME: everything its scope owns goes."
  (let ((p (find-package name)))
    (when p
      (hash-table-delete! %packages name)
      (scope-shutdown! (package-scope p)))))
