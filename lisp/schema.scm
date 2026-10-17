;;; Schemas: what values may be, written as data.
;;;
;;; A schema is a plain value, quoted or built with quasiquote, so schemas
;;; are reused by naming them as any value is, and composed by embedding:
;;;
;;;   (define port '(natural #:min 1 #:max 65535))
;;;   (define server
;;;     `(dict (host string #:default "127.0.0.1")
;;;            (port ,port #:doc "Where to listen.")))
;;;
;;; The schemas:
;;;
;;; - any, boolean, string, symbol, keyword, char, integer (exact), natural,
;;;   number, real, bytevector, procedure. The numbers and string take
;;;   bounds in a list, as (natural #:min 1 #:max 65535): of the number, of
;;;   a string's length.
;;; - (one-of value ...): one of the values, as `equal?` compares them.
;;; - (or schema ...): a value of one of the schemas.
;;; - (list-of schema), (vector-of schema): each item of that schema;
;;;   #:min and #:max bound the length.
;;; - (dict-of key-schema value-schema): a dict, any keys and values of
;;;   those schemas.
;;; - (dict (key schema option ...) ...): a dict with those keys and no
;;;   others. Options: #:optional; #:default VALUE, which makes it optional
;;;   too; #:doc TEXT.
;;;
;;; `valid?` checks a value strictly. `explain` says what is wrong with
;;; it: each problem a dict of its `path` (the keys and indexes from the
;;; value to the part that is wrong), `message`, and `value` unless it is
;;; missing. `conform` turns data read from a file or JSON into the value
;;; the schema describes: it fills in defaults, and converts what is not
;;; valid as it is but unambiguous, a string to the symbol the schema wants
;;; (keys too), a vector to a list and back. `schema->json-schema` gives
;;; the JSON Schema, for agents and other tools.

(provide valid? explain conform problems->string schema? schema-description schema-choices schema->json-schema)

;; Also the library (techne schema) (lisp/techne/schema.sld).
(%name-library '(techne schema))

;; name, predicate, description, what #:min and #:max bound (#f: none)
(define %atoms
  `((any ,(lambda (v) #t) "anything" #f)
    (boolean ,boolean? "a boolean" #f)
    (string ,string? "a string" length)
    (symbol ,symbol? "a symbol" #f)
    (keyword ,keyword? "a keyword" #f)
    (char ,char? "a character" #f)
    (integer ,exact-integer? "an integer" value)
    (natural ,(lambda (v) (and (exact-integer? v) (>= v 0))) "a natural number" value)
    (number ,number? "a number" value)
    (real ,real? "a real number" value)
    (bytevector ,bytevector? "a bytevector" #f)
    (procedure ,procedure? "a procedure" #f)))

(define (written v)
  (let ((s (call-with-output-string (lambda (p) (write v p)))))
    (if (> (string-length s) 60) (string-append (substring s 0 57) "...") s)))

;; A schema as a list: a bare name is the name without options.
(define (%form s)
  (cond ((symbol? s) (list s))
        ((and (pair? s) (symbol? (car s)) (list? s)) s)
        (else (error "Not a schema:" s))))

;; The options in ARGS: (name . value) by keyword name, a flag's value #t.
(define (%options args flags)
  (let loop ((args args) (acc '()))
    (cond ((null? args) (reverse acc))
          ((not (keyword? (car args))) (error "Not a schema option:" (car args)))
          ((memq (car args) flags) (loop (cdr args) (cons (cons (keyword->string (car args)) #t) acc)))
          ((null? (cdr args)) (error "A schema option without its value:" (car args)))
          (else (loop (cddr args) (cons (cons (keyword->string (car args)) (cadr args)) acc))))))

(define (%option opts name [default #f])
  (let ((o (assoc name opts))) (if o (cdr o) default)))

;; The bounds of ARGS (#:min and #:max), checked: (min . max), each #f
;; without one. KIND is what they bound: `value`, `length` or #f (none).
(define (%bounds args kind)
  (let ((opts (%options args '())))
    (for-each (lambda (o)
                (unless (and kind (member (car o) '("min" "max"))) (error "Not a schema option:" (car o)))
                (unless (if (eq? kind 'length) (and (exact-integer? (cdr o)) (>= (cdr o) 0)) (real? (cdr o)))
                  (error "Not a bound:" (cdr o))))
              opts)
    (cons (%option opts "min") (%option opts "max"))))

(define (%in-bounds? n bounds)
  (and (or (not (car bounds)) (>= n (car bounds))) (or (not (cdr bounds)) (<= n (cdr bounds)))))

(define (%bounds-text bounds)
  (let ((lo (car bounds)) (hi (cdr bounds)))
    (cond ((and lo hi) (string-append " from " (written lo) " to " (written hi)))
          (lo (string-append " at least " (written lo)))
          (hi (string-append " at most " (written hi)))
          (else ""))))

;; A dict schema's entries, checked: (key schema optional default doc),
;; DEFAULT a list of the value, or '().
(define (%entries s)
  (let ((entries
         (map (lambda (e)
                (unless (and (list? e) (>= (length e) 2)) (error "Not a dict schema entry:" e))
                (let* ((opts (%options (cddr e) '(#:optional)))
                       (default (assoc "default" opts))
                       (doc (%option opts "doc")))
                  (for-each (lambda (o)
                              (unless (member (car o) '("optional" "default" "doc")) (error "Not a dict entry option:" (car o))))
                            opts)
                  (unless (or (not doc) (string? doc)) (error "Not a documentation string:" doc))
                  (list (car e) (cadr e) (or (%option opts "optional") (and default #t)) (if default (list (cdr default)) '()) doc)))
              (cdr s))))
    (let loop ((keys (map car entries)))
      (cond ((null? keys))
            ((member (car keys) (cdr keys)) (error "A dict schema key twice:" (car keys)))
            (else (loop (cdr keys)))))
    entries))

(define (schema-description s)
  "Return a short description of the values of the schema S.
It reads as \"expected ...\" would go on, as in \"a natural number from
1 to 65535\"."
  (let* ((f (%form s)) (head (car f)) (args (cdr f)) (atom (assq head %atoms)))
    (cond (atom
           (let ((b (%bounds args (cadddr atom))))
             (string-append (caddr atom)
                            (if (and (eq? (cadddr atom) 'length) (or (car b) (cdr b))) " of length" "")
                            (%bounds-text b))))
          ((eq? head 'one-of)
           (if (= (length args) 1)
               (written (car args))
               (string-append "one of " (string-join (map written args) ", "))))
          ((eq? head 'or) (string-join (map schema-description args) " or "))
          ((memq head '(list-of vector-of))
           (string-append (if (eq? head 'list-of) "a list" "a vector")
                          (let ((b (%bounds (cdr args) 'length)))
                            (if (or (car b) (cdr b)) (string-append " of length" (%bounds-text b)) ""))
                          ", each " (schema-description (car args))))
          ((eq? head 'dict-of)
           (string-append "a dict from " (schema-description (car args)) " to " (schema-description (cadr args))))
          ((eq? head 'dict)
           (let ((keys (map (lambda (e) (written (car e))) (%entries f))))
             (if (null? keys) "an empty dict" (string-append "a dict with the keys " (string-join keys ", ")))))
          (else (error "Not a schema:" s)))))

(define (%problem path message . value)
  (apply dict 'path (reverse path) 'message message (if (null? value) '() (list 'value (car value)))))

(define (%expected s v path)
  (list (%problem path (string-append "expected " (schema-description s) ", got " (written v)) v)))

;; Walk V with schema S at PATH (reversed): (value . problems). When
;; COERCE, VALUE has its defaults filled in, and a part not valid as it
;; is converted where that is unambiguous.
(define (%walk s v path coerce)
  (let* ((f (%form s)) (head (car f)) (args (cdr f)) (atom (assq head %atoms)))
    (cond
     (atom
      (let ((v (if (and coerce (eq? head 'symbol) (string? v)) (string->symbol v) v))
            (b (%bounds args (cadddr atom))))
        (cons v (if (and ((cadr atom) v)
                         (case (cadddr atom)
                           ((length) (%in-bounds? (string-length v) b))
                           ((value) (%in-bounds? v b))
                           (else #t)))
                    '()
                    (%expected s v path)))))
     ((eq? head 'one-of)
      (let ((v (if (and coerce (string? v) (not (member v args)) (memq (string->symbol v) args)) (string->symbol v) v)))
        (cons v (if (member v args) '() (%expected s v path)))))
     ((eq? head 'or)
      ;; The first branch V is valid in as it is, else the first it can
      ;; be made valid in.
      (let ((valid (find (lambda (b) (null? (cdr (%walk b v path #f)))) args)))
        (if valid
            (%walk valid v path coerce)
            (let loop ((branches (if coerce args '())))
              (if (null? branches)
                  (cons v (%expected s v path))
                  (let ((r (%walk (car branches) v path #t)))
                    (if (null? (cdr r)) r (loop (cdr branches)))))))))
     ((memq head '(list-of vector-of))
      (let* ((v (cond ((not coerce) v)
                      ((and (eq? head 'list-of) (vector? v)) (vector->list v))
                      ((and (eq? head 'vector-of) (list? v)) (list->vector v))
                      (else v)))
             (items (cond ((and (eq? head 'list-of) (list? v)) v)
                          ((and (eq? head 'vector-of) (vector? v)) (vector->list v))
                          (else #f)))
             (b (%bounds (cdr args) 'length)))
        (if (not (and items (%in-bounds? (length items) b)))
            (cons v (%expected s v path))
            (let ((rs (map (lambda (x i) (%walk (car args) x (cons i path) coerce)) items (iota (length items)))))
              (cons (if (eq? head 'list-of) (map car rs) (list->vector (map car rs)))
                    (append-map cdr rs))))))
     ((eq? head 'dict-of)
      (if (not (dict? v))
          (cons v (%expected s v path))
          ;; A key is converted only into one the dict does not have.
          (let ((rs (map (lambda (k)
                           (let* ((key (%walk (car args) k (cons k path) coerce))
                                  (key (if (and (not (equal? (car key) k)) (dict-contains? v (car key)))
                                           (%walk (car args) k (cons k path) #f)
                                           key)))
                             (cons key (%walk (cadr args) (dict-ref v k) (cons k path) coerce))))
                         (dict-keys v))))
            (cons (fold (lambda (r d) (dict-set d (car (car r)) (car (cdr r)))) (dict) rs)
                  (append-map (lambda (r) (append (cdr (car r)) (cdr (cdr r)))) rs)))))
     ((eq? head 'dict)
      (if (not (dict? v))
          (cons v (%expected s v path))
          (let* ((entries (%entries f))
                 (known (map car entries))
                 ;; A string key read from JSON is the symbol the schema
                 ;; has, unless the string is a key of the schema too, or
                 ;; the dict has the symbol already.
                 (key (lambda (k)
                        (if (and coerce (string? k) (not (member k known)) (memq (string->symbol k) known)
                                 (not (dict-contains? v (string->symbol k))))
                            (string->symbol k)
                            k)))
                 (given (map (lambda (k) (cons (key k) (dict-ref v k))) (dict-keys v)))
                 (unknown (filter (lambda (kv) (not (member (car kv) known))) given))
                 ;; Each entry's (key value problems), VALUE a list of the
                 ;; value, or '() when it has none.
                 (rs (map (lambda (e)
                            (let* ((kv (assoc (car e) given))
                                   (default (cadddr e))
                                   (walk (lambda (x)
                                           (let ((r (%walk (cadr e) x (cons (car e) path) coerce)))
                                             (list (car e) (list (car r)) (cdr r))))))
                              (cond (kv (walk (cdr kv)))
                                    ((and coerce (pair? default)) (walk (car default)))
                                    ((caddr e) (list (car e) '() '()))
                                    (else (list (car e) '() (list (%problem (cons (car e) path) "missing")))))))
                          entries)))
            (cons (fold (lambda (r d) (if (pair? (cadr r)) (dict-set d (car r) (car (cadr r))) d))
                        (fold (lambda (kv d) (dict-set d (car kv) (cdr kv))) (dict) unknown)
                        rs)
                  (append (append-map caddr rs)
                          (map (lambda (kv) (%problem (cons (car kv) path) "unknown key" (cdr kv))) unknown))))))
     (else (error "Not a schema:" s)))))

(define (valid? schema value)
  "Return #t if VALUE is a value of SCHEMA, as it is, without conversion."
  (null? (cdr (%walk schema value '() #f))))

(define (explain schema value)
  "Return the problems of VALUE as a value of SCHEMA, as a list.
Each is a dict of its `path`, `message`, and `value` unless missing. The
list is empty when VALUE is valid."
  (cdr (%walk schema value '() #f)))

(define (problems->string problems)
  "Return PROBLEMS, as `explain` returns them, as lines of text."
  (string-join
   (map (lambda (p)
          (let ((path (dict-ref p 'path)))
            (string-append "  "
                           (if (null? path)
                               ""
                               (string-append
                                (fold (lambda (k acc)
                                        (cond ((exact-integer? k) (string-append acc "[" (number->string k) "]"))
                                              ((string=? acc "") (if (symbol? k) (symbol->string k) (written k)))
                                              (else (string-append acc "." (if (symbol? k) (symbol->string k) (written k))))))
                                      "" path)
                                ": "))
                           (dict-ref p 'message))))
        problems)
   "\n"))

(define (conform schema value #:what [what "value"] #:else [fail #f])
  "Return VALUE made a value of SCHEMA, else raise an error saying why.
Defaults are filled in. A part not valid as it is is converted where that
is unambiguous: a string to the symbol SCHEMA wants, as a key too, a
vector to the list it wants and a list to the vector. WHAT names the
value in the error, as in \"Invalid WHAT:\". With FAIL, it returns what
FAIL returns for the problems instead, as `explain` gives them."
  (let ((r (%walk schema value '() #t)))
    (cond ((null? (cdr r)) (car r))
          (fail (fail (cdr r)))
          (else (error (string-append "Invalid " what ":\n" (problems->string (cdr r))))))))

(define (schema? x)
  "Return #t if X is a schema, whose defaults are values of their schemas."
  (guard (e ((error-object? e) #f))
    (let check ((s x))
      (let* ((f (%form s)) (head (car f)) (args (cdr f)) (atom (assq head %atoms)))
        (cond (atom (%bounds args (cadddr atom)))
              ((eq? head 'one-of))
              ((eq? head 'or)
               (when (null? args) (error "Not a schema:" s))
               (for-each check args))
              ((memq head '(list-of vector-of))
               (unless (pair? args) (error "Not a schema:" s))
               (check (car args))
               (%bounds (cdr args) 'length))
              ((eq? head 'dict-of)
               (unless (= (length args) 2) (error "Not a schema:" s))
               (for-each check args))
              ((eq? head 'dict)
               (for-each (lambda (e)
                           (check (cadr e))
                           (when (and (pair? (cadddr e)) (not (valid? (cadr e) (car (cadddr e)))))
                             (error "Not a valid default:" (car (cadddr e)))))
                         (%entries f)))
              (else (error "Not a schema:" s)))))
    #t))

(define (schema-choices schema)
  "Return the values SCHEMA lists: those of `one-of` and booleans.
For a schema with finitely many values that is all of them, as a prompt
offers them."
  (let* ((f (%form schema)) (head (car f)))
    (cond ((eq? head 'boolean) '(#t #f))
          ((eq? head 'one-of) (cdr f))
          ((eq? head 'or) (append-map schema-choices (cdr f)))
          (else '()))))

;; V as JSON data, as `conform` reads it back: symbols but `null`,
;; keywords and characters as strings.
(define (%json-value v)
  (cond ((eq? v 'null) v)
        ((symbol? v) (symbol->string v))
        ((keyword? v) (keyword->string v))
        ((char? v) (string v))
        ((or (string? v) (boolean? v) (and (real? v) (exact? v) (integer? v)) (and (real? v) (inexact? v))) v)
        ((list? v) (list->vector (map %json-value v)))
        ((vector? v) (vector-map %json-value v))
        ((dict? v) (fold (lambda (k d) (dict-set d (%json-value k) (%json-value (dict-ref v k)))) (dict) (dict-keys v)))
        (else (error "No JSON form for:" v))))

(define (schema->json-schema schema)
  "Return the JSON Schema of SCHEMA's values, as data for `json->string`.
Symbols, keywords and characters are strings in JSON; bytevectors and
procedures have no JSON form, and are an error."
  (unless (schema? schema) (error "Not a schema:" schema))
  (let* ((f (%form schema)) (head (car f)) (args (cdr f))
         (bounds (lambda (kind lo hi)
                   (let ((b (%bounds (if (memq head '(list-of vector-of)) (cdr args) args) kind)))
                     (append (if (car b) (list lo (car b)) '()) (if (cdr b) (list hi (cdr b)) '()))))))
    (case head
      ((any) (dict))
      ((boolean) (dict "type" "boolean"))
      ((string) (apply dict "type" "string" (bounds 'length "minLength" "maxLength")))
      ((symbol keyword) (dict "type" "string"))
      ((char) (dict "type" "string" "minLength" 1 "maxLength" 1))
      ((integer natural)
       (let* ((b (%bounds args 'value))
              (lo (if (eq? head 'natural) (max 0 (or (car b) 0)) (car b))))
         (apply dict "type" "integer" (append (if lo (list "minimum" lo) '()) (if (cdr b) (list "maximum" (cdr b)) '())))))
      ((number real) (apply dict "type" "number" (bounds 'value "minimum" "maximum")))
      ((one-of) (dict "enum" (%json-value args)))
      ((or) (dict "anyOf" (list->vector (map schema->json-schema args))))
      ((list-of vector-of)
       (apply dict "type" "array" "items" (schema->json-schema (car args)) (bounds 'length "minItems" "maxItems")))
      ((dict-of)
       (dict "type" "object" "propertyNames" (schema->json-schema (car args)) "additionalProperties" (schema->json-schema (cadr args))))
      ((dict)
       (let ((entries (%entries f)))
         (dict "type" "object"
               "properties"
               (fold (lambda (e d)
                       (let* ((p (schema->json-schema (cadr e)))
                              (p (if (pair? (cadddr e)) (dict-set p "default" (%json-value (car (cadddr e)))) p))
                              (p (if (list-ref e 4) (dict-set p "description" (list-ref e 4)) p)))
                         (dict-set d (%json-value (car e)) p)))
                     (dict) entries)
               "required" (list->vector (filter-map (lambda (e) (and (not (caddr e)) (%json-value (car e)))) entries))
               "additionalProperties" #f)))
      (else (error "No JSON Schema for:" schema)))))
