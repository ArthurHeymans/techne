(require "../test.scm")
(require "../schema.scm")

(define port '(natural #:min 1 #:max 65535))
(define server
  `(dict (name symbol #:doc "What the server is called.")
         (host string #:default "127.0.0.1")
         (port ,port)
         (tags (list-of string) #:optional)))

(define (messages schema value)
  (map (lambda (p) (list (dict-ref p 'path) (dict-ref p 'message))) (explain schema value)))

(define (failure thunk)
  (guard (e ((error-object? e) (error-object-message e))) (thunk) #f))

;; Atoms and bounds
(check-true "natural" (valid? 'natural 0))
(check "not natural" #f (valid? 'natural -1))
(check "inexact is not an integer" #f (valid? 'integer 1.0))
(check-true "bounded" (valid? port 80))
(check "out of bounds" '((() "expected a natural number from 1 to 65535, got 0")) (messages port 0))
(check "string length" '((() "expected a string of length at least 1, got \"\"")) (messages '(string #:min 1) ""))

;; Choices
(check-true "one-of" (valid? '(one-of #f absolute relative) 'relative))
(check "one-of message" '((() "expected one of #f, absolute, relative, got 3")) (messages '(one-of #f absolute relative) 3))
(check-true "or" (valid? '(or (one-of #f) natural) 3))
(check "or message" '((() "expected #f or a natural number, got x")) (messages '(or (one-of #f) natural) 'x))
(check "choices" '(#f absolute relative #t #f) (schema-choices '(or (one-of #f absolute relative) boolean)))

;; Collections, with paths to what is wrong
(check "list item" '(((2) "expected a string, got 3")) (messages '(list-of string) '("a" "b" 3)))
(check "vector is not a list" #f (valid? '(list-of string) #("a")))
(check "list length" '((() "expected a list of length at most 1, each a string, got (\"a\" \"b\")"))
       (messages '(list-of string #:max 1) '("a" "b")))
(check "dict-of" '(((b) "expected a natural number, got -1")) (messages '(dict-of symbol natural) (dict 'a 1 'b -1)))

;; Dicts: missing, unknown and nested problems all at once
(check-true "server" (valid? server (dict 'name 'web 'port 8080)))
(check "server problems"
       '(((name) "missing") ((port) "expected a natural number from 1 to 65535, got 0") ((tags 0) "expected a string, got 1")
         ((prot) "unknown key"))
       (messages server (dict 'port 0 'tags '(1) 'prot 3)))

;; Conforming data from JSON: string keys and symbols, arrays, defaults
(define read (conform server (string->json "{\"name\": \"web\", \"port\": 8080, \"tags\": [\"a\"]}")))
(check "conformed" '(web "127.0.0.1" 8080 ("a")) (map (lambda (k) (dict-ref read k)) '(name host port tags)))
(check "an optional entry stays out" #f (dict-contains? (conform server (dict 'name 'web 'port 1)) 'tags))
(check "a value given is kept" "::" (dict-ref (conform server (dict 'name 'web 'port 1 'host "::")) 'host))
(check "one-of from a string" 'relative (conform '(one-of #f absolute relative) "relative"))
(check "a list to a vector" #(1 2) (conform '(vector-of natural) '(1 2)))
(check "dict-of keys" 2 (dict-ref (conform '(dict-of symbol natural) (string->json "{\"a\": 2}")) 'a))
(check "conform failure"
       "Invalid server:\n  name: missing\n  port: expected a natural number from 1 to 65535, got 70000"
       (failure (lambda () (conform server (dict 'port 70000) #:what "server"))))
(check "a string is not coerced to a number" #f (valid? 'natural "1"))
(check "problems as data" '((name)) (conform server (dict 'port 1) #:else (lambda (ps) (map (lambda (p) (dict-ref p 'path)) ps))))
(check "defaults are conformed" '(a) (dict-ref (conform '(dict (k (list-of symbol) #:default #("a"))) (dict)) 'k))

;; What is valid as it is stays as it is
(check "a valid string stays" "a" (conform '(or symbol string) "a"))
(check "a valid one-of string stays" "a" (conform '(one-of a "a") "a"))
(check "a string key of the schema stays" 2 (dict-ref (conform '(dict (a natural #:optional) ("a" natural)) (dict "a" 2)) "a"))
(check "no key is lost to conversion" '((("a") "expected a symbol, got \"a\""))
       (conform '(dict-of symbol natural) (dict 'a 1 "a" 2) #:else (lambda (ps) (map (lambda (p) (list (dict-ref p 'path) (dict-ref p 'message))) ps))))

;; Schemas themselves
(check-true "schema" (schema? server))
(check "not a schema" #f (schema? '(list-of)))
(check "bad option" #f (schema? '(natural #:least 1)))
(check "bad entry" #f (schema? '(dict (a))))
(check "a bound of nothing" #f (schema? '(boolean #:min 4)))
(check "a negative length" #f (schema? '(list-of any #:min -1)))
(check "an empty or" #f (schema? '(or)))
(check "a key twice" #f (schema? '(dict (a any) (a any))))
(check "an invalid default" #f (schema? '(dict (a natural #:default "oops"))))
(check "description" "a dict with the keys name, host, port, tags" (schema-description server))

;; JSON Schema
(check "json schema"
       "{\"additionalProperties\":false,\"properties\":{\"host\":{\"default\":\"127.0.0.1\",\"type\":\"string\"},\"name\":{\"description\":\"What the server is called.\",\"type\":\"string\"},\"port\":{\"maximum\":65535,\"minimum\":1,\"type\":\"integer\"},\"tags\":{\"items\":{\"type\":\"string\"},\"type\":\"array\"}},\"required\":[\"name\",\"port\"],\"type\":\"object\"}"
       (json->string (schema->json-schema server)))
(check "json enum" "{\"enum\":[false,\"absolute\",null,[\"a\"]]}"
       (json->string (schema->json-schema '(one-of #f absolute null (a)))))
(check "json keys" "{\"additionalProperties\":{\"type\":\"integer\"},\"propertyNames\":{\"enum\":[\"a\",\"b\"]},\"type\":\"object\"}"
       (json->string (schema->json-schema '(dict-of (one-of a b) integer))))

(test-summary)
