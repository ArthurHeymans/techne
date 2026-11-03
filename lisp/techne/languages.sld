;;; Default language data shared by the editor and headless source tools.
(define-library (techne languages)
 (export builtin-languages source-language language-value captured-definitions)
 (import (techne))
 (begin
 (define builtin-languages '()
   "Default language declarations shared by editor modes and source tools.")
 (define (language-value l key)
   "Return KEY in the language declaration L, or #f."
   (let ((entry (assq key l))) (and entry (cadr entry))))
 (define (%declare name doc #:grammar [grammar name] #:files [files '()] #:highlights [highlights #f] #:last-wins [last-wins #f] #:definitions [definitions #f])
   (set! builtin-languages (append builtin-languages (list `((name ,name) (doc ,doc) (grammar ,grammar) (files ,files) (highlights ,highlights) (last-wins ,last-wins) (definitions ,definitions))))))
 (define-syntax define-language (syntax-rules () ((_ name doc args ...) (%declare 'name doc args ...))))
;;; The languages built in, as the runtime's grammars are.

(define-language rust "Rust." #:files '(".rs")
  #:definitions "[(function_item name: (_) @name) (function_signature_item name: (_) @name)
                  (struct_item name: (_) @name) (enum_item name: (_) @name) (union_item name: (_) @name)
                  (trait_item name: (_) @name) (impl_item type: (_) @name) (mod_item name: (_) @name)
                  (macro_definition name: (_) @name) (const_item name: (_) @name) (static_item name: (_) @name)
                  (type_item name: (_) @name)] @definition")
(define-language python "Python." #:files '(".py" ".pyi") #:last-wins #t
  #:definitions "[(function_definition name: (_) @name) (class_definition name: (_) @name)] @definition")
(define-language c "C." #:files '(".c" ".h") #:last-wins #t
  #:definitions "[(function_definition declarator: (function_declarator declarator: (_) @name))
                  (struct_specifier name: (_) @name body: (_)) (enum_specifier name: (_) @name body: (_))
                  (type_definition declarator: (_) @name)] @definition")
(define js-definitions
  "[(function_declaration name: (_) @name) (generator_function_declaration name: (_) @name)
    (class_declaration name: (_) @name) (method_definition name: (_) @name)
    (lexical_declaration (variable_declarator name: (_) @name value: [(arrow_function) (function_expression)]))] @definition")
(define-language javascript "JavaScript and JSX." #:files '(".js" ".mjs" ".cjs" ".jsx") #:last-wins #t
  #:definitions js-definitions)
(define ts-definitions
  "[(function_declaration name: (_) @name) (generator_function_declaration name: (_) @name)
    (class_declaration name: (_) @name) (abstract_class_declaration name: (_) @name) (method_definition name: (_) @name)
    (interface_declaration name: (_) @name) (type_alias_declaration name: (_) @name) (enum_declaration name: (_) @name)
    (lexical_declaration (variable_declarator name: (_) @name value: [(arrow_function) (function_expression)]))] @definition")
(define-language typescript "TypeScript." #:files '(".ts" ".mts" ".cts") #:last-wins #t #:definitions ts-definitions)
(define-language tsx "TypeScript with JSX." #:files '(".tsx") #:last-wins #t #:definitions ts-definitions)
(define-language go "Go." #:files '(".go")
  #:definitions "[(function_declaration name: (_) @name) (method_declaration name: (_) @name)
                  (type_declaration (type_spec name: (_) @name))] @definition")
(define-language json "JSON." #:files '(".json"))
(define-language yaml "YAML." #:files '(".yaml" ".yml") #:last-wins #t)
(define-language toml "TOML." #:files '(".toml") #:last-wins #t
  #:definitions "[(table [(bare_key) (dotted_key) (quoted_key)] @name) (table_array_element [(bare_key) (dotted_key) (quoted_key)] @name)] @definition")
(define-language scheme "Scheme and Techne Lisp." #:files '(".scm" ".sld" ".sls" ".ss") #:last-wins #t
  ;; A top-level form whose head is a defining form's (define,
  ;; define-record-type, define-command...), not any call.
  #:definitions "((program (list . (symbol) @_head . [(symbol) @name (list . (symbol) @name)]) @definition) (#match? @_head \"^def\"))")
(define-language bash "Shell scripts." #:files '(".sh" ".bash") #:last-wins #t
  #:definitions "(function_definition name: (_) @name) @definition")
(define-language markdown "Markdown." #:files '(".md" ".markdown") #:last-wins #t
  #:definitions "(section (atx_heading (inline) @name)) @definition")

(define (source-language path)
 "Return the built-in language declaration for PATH, or #f."
 (let ((best (fold (lambda (l best) (fold (lambda (suffix best) (if (and (string-suffix? suffix path) (or (not best) (> (string-length suffix) (cdr best)))) (cons l (string-length suffix)) best)) best (language-value l 'files))) #f builtin-languages))) (and best (car best))))
;; Definitions as a query's captures CS give them: (start end name).
(define (captured-definitions slice cs)
  "Group definition/name captures CS into (start end name) entries.
SLICE reads a character range from the source being queried."
  ;; Captures private to the query (@_head) are its own business.
  (let loop ((cs (filter (lambda (c) (not (string-prefix? "_" (car c)))) cs)) (acc '()))
    (cond ((null? cs) (reverse acc))
          ((string=? (car (car cs)) "definition")
           ;; Its name, if the next capture is one inside it.
           (let* ((c (car cs)) (next (and (pair? (cdr cs)) (cadr cs)))
                  (name (and next (string=? (car next) "name") (<= (cadr c) (cadr next)) (<= (caddr next) (caddr c)) next)))
             (loop (if name (cddr cs) (cdr cs))
                   (cons (list (cadr c) (caddr c)
                               (and name (slice (cadr name) (caddr name))))
                         acc))))
          (else (loop (cdr cs) acc)))))

))
