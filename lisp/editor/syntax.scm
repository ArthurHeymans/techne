;;; Syntax: the languages buffers are parsed in, and moving by their
;;; trees (EDITOR.md, section 3).
;;;
;;; A language is a grammar the runtime has built in, the file names it
;;; is for, its highlight queries (the grammar's own unless given), and
;;; a query capturing its definitions: what C-M-a and C-M-e move
;;; between, C-M-h marks and imenu lists. Languages are declared with
;;; `define-language', scope-owned as modes are, so a package can add
;;; one or change another's queries. The runtime parses a buffer in its
;;; language (`buffer-language') and colours it from the queries'
;;; captures, under the buffer's own layers.
;;;
;;; The commands are Emacs's for its tree-sitter modes: C-M-f and C-M-b
;;; move over a node, C-M-u out of one, C-M-d into one, C-M-k kills
;;; one; C-M-a and C-M-e go to a definition's start and end, C-M-h marks
;;; it; C-= selects the node at point, then the one around it, as
;;; expand-region does. Lisp sees nodes as (kind start end named?), and
;;; can ask a document's tree what it holds (`syntax-query').

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "minibuffer.scm")
(require "targets.scm")
(require "faces.scm")
(require "requests.scm")

(provide define-language register-language! find-language language-names language-grammar language-files
         language-definitions path-language buffer-language
         node-at node-kind node-start node-end node-named? node-parent node-children syntax-captures
         forward-sexp backward-sexp backward-up-list down-list kill-sexp mark-sexp
         beginning-of-defun end-of-defun mark-defun expand-region imenu describe-syntax-at-point)

;;; Languages

(define-record-type language
  (%make-language name doc grammar files highlights last-wins definitions)
  language?
  (name language-name)
  (doc language-doc)
  ;; A grammar of `syntax-grammars', a symbol.
  (grammar language-grammar)
  ;; File name suffixes.
  (files language-files)
  ;; Highlight queries' text, or #f for the grammar's own.
  (highlights language-highlights)
  ;; Whether of the patterns matching a node the last wins it.
  (last-wins language-last-wins)
  ;; A query capturing definitions as @definition, their names as
  ;; @name, or #f.
  (definitions language-definitions))

;; The runtime learns of a language when the one in effect for its name
;; changes, so a package's language goes when the package does.
(define %languages
  (make-registry 'languages
                 #:changed (lambda (name l)
                             (if l
                                 (syntax-define-language! (symbol->string name) (symbol->string (language-grammar l))
                                                          (language-highlights l) (language-last-wins l))
                                 (syntax-forget-language! (symbol->string name))))))

(define (find-language name)
  "Return the language NAME, or #f."
  (registry-ref %languages name))

(define (language-names)
  "Return the names of the languages defined."
  (registry-keys %languages))

(define (register-language! name doc #:grammar [grammar name] #:files [files '()] #:highlights [highlights #f]
                            #:last-wins [last-wins #f] #:definitions [definitions #f])
  "Define the language NAME, documented by DOC.
GRAMMAR is the grammar it is parsed with, one of `syntax-grammars'
as a symbol; FILES the file name suffixes it is for; HIGHLIGHTS the
text of its highlight queries, #f for the grammar's own, each
capture naming the face its node is drawn with. Of the patterns
matching a node, the first wins it, or the last with LAST-WINS true,
as the queries are written for. DEFINITIONS is a query capturing its
definitions as @definition and their names as @name, for C-M-a,
C-M-e, C-M-h and imenu. A grammar the runtime does not have, or a
query that does not compile, is an error."
  (unless (member (symbol->string grammar) (syntax-grammars)) (error "define-language: no such grammar:" grammar))
  ;; Compiled now, so a bad query fails here rather than where the
  ;; runtime takes the language.
  (syntax-check-language (symbol->string grammar) highlights definitions)
  (registry-add! %languages name (%make-language name doc grammar files highlights last-wins definitions))
  name)

(define-syntax define-language
  (syntax-rules ()
    "Define the language NAME, documented by DOC.
The other arguments are keywords, as `register-language!' takes them."
    ((_ name doc arg ...) (register-language! 'name doc arg ...))))

(define (path-language path)
  "Return the name of the language of the file at PATH, or #f.
That is the language with the longest file suffix PATH has."
  (and path
       (let ((best (fold (lambda (name best)
                           (fold (lambda (suffix best)
                                   (if (and (string-suffix? suffix path) (or (not best) (> (string-length suffix) (cdr best))))
                                       (cons name (string-length suffix))
                                       best))
                                 best (language-files (find-language name))))
                         #f (language-names))))
         (and best (car best)))))

(define (buffer-language b)
  "Return the name of buffer B's language, or #f.
Its `syntax-language' option says, else its file's name."
  (and b (let ((l (or (option b 'syntax-language) (path-language (document-path (buffer-document b))))))
           (and l (find-language l) l))))

;;; Nodes

(define (node-kind n) "Return the kind of the syntax node N, a string." (car n))
(define (node-start n) "Return where the syntax node N starts." (cadr n))
(define (node-end n) "Return where the syntax node N ends." (caddr n))
(define (node-named? n) "Return #t if the syntax node N is named, not a token." (cadddr n))

;; The language of buffer B, or an error saying there is none.
(define (language-of b)
  (let ((l (and b (buffer-language b))))
    (or l (error "No syntax tree here: the buffer has no language"))))

(define (node-at b from #:to [to from] #:named [named #f])
  "Return the smallest syntax node of buffer B holding FROM to TO.
Only named nodes when NAMED; #f when there is none."
  (syntax-node-at (buffer-document b) (symbol->string (language-of b)) from to named))

(define (node-parent b n)
  "Return the parent of the syntax node N of buffer B, or #f."
  (syntax-node-parent (buffer-document b) (symbol->string (language-of b)) n))

(define (node-children b n)
  "Return the children of the syntax node N of buffer B, in order."
  (syntax-node-children (buffer-document b) (symbol->string (language-of b)) n))

(define (syntax-captures b query #:from [from 0] #:to [to (document-length (buffer-document b))])
  "Return the captures of QUERY, a query's text, over buffer B's tree.
Each is (capture start end), between FROM and TO, in order."
  (syntax-query (buffer-document b) (symbol->string (language-of b)) query from to))

;;; Moving by nodes

;; The node at P to move over forward: the biggest one starting at P
;; that is not the root, or the next child after P of the node around it.
(define (next-node b p)
  (let* ((len (document-length (buffer-document b)))
         (leaf (node-at b p #:to (min len (+ p 1)))))
    (cond ((not leaf) #f)
          ((>= (node-start leaf) p) (widest-from b leaf node-start))
          ((node-has-children? b leaf) (node-child-near b leaf p #t))
          (else leaf))))

;; The node at P to move over backward: the biggest one ending at P, or
;; the last child before P of the node around it.
(define (previous-node b p)
  (let ((leaf (and (> p 0) (node-at b (- p 1) #:to p))))
    (cond ((not leaf) #f)
          ((<= (node-end leaf) p) (widest-from b leaf node-end))
          ((node-has-children? b leaf) (node-child-near b leaf p #f))
          (else leaf))))

(define (node-has-children? b n)
  (> (syntax-node-child-count (buffer-document b) (symbol->string (language-of b)) n) 0))

;; N's child nearest P: the first starting at or after it, or (not
;; FORWARD) the last ending at or before it.
(define (node-child-near b n p forward)
  (syntax-node-child-near (buffer-document b) (symbol->string (language-of b)) n p forward))

;; N, or its biggest ancestor sharing its EDGE (start or end), the root
;; aside.
(define (widest-from b n edge)
  (let loop ((n n))
    (let ((up (node-parent b n)))
      (if (and up (node-parent b up) (= (edge up) (edge n))) (loop up) n))))

(define (sexp-end b p n)
  (let ((len (document-length (buffer-document b))))
    (let loop ((p p) (k n))
      (if (= k 0) p
          (let ((node (and (< p len) (next-node b p))))
            (if node (loop (min len (max (node-end node) (+ p 1))) (- k 1)) (error "No next expression")))))))

(define (sexp-start b p n)
  (let loop ((p p) (k n))
    (if (= k 0) p
        (let ((node (and (> p 0) (previous-node b p))))
          (if node (loop (max 0 (min (node-start node) (- p 1))) (- k 1)) (error "No previous expression"))))))

(define (at-buffer s) (or (current-buffer s) (error "No buffer")))

(define-command (forward-sexp s n)
  "Move over the next syntax node: an expression, a statement, a list."
  (let ((b (at-buffer s)))
    (move! s (lambda (p) (if (< n 0) (sexp-start b p (- n)) (sexp-end b p n))))))

(define-command (backward-sexp s n)
  "Move back over the previous syntax node."
  (let ((b (at-buffer s)))
    (move! s (lambda (p) (if (< n 0) (sexp-end b p (- n)) (sexp-start b p n))))))

(define-command (backward-up-list s n)
  "Move to the start of the syntax node around point."
  (let ((b (at-buffer s)))
    (move! s (lambda (p)
               (let loop ((node (node-at b p)) (k n))
                 (cond ((not node) (error "At top level"))
                       ((and (< (node-start node) p) (= k 1) (node-parent b node)) (node-start node))
                       ((< (node-start node) p) (loop (node-parent b node) (- k 1)))
                       (else (loop (node-parent b node) k))))))))

(define-command (down-list s n)
  "Move into the next syntax node that holds others, past its opening.
With a count, that many levels down; a negative count goes backward,
into the node before point, before its closing."
  (let ((b (at-buffer s)))
    (move! s (lambda (p)
               (let level ((p p) (k (abs n)))
                 (if (= k 0)
                     p
                     (level (if (< n 0) (down-backward b p) (down-forward b p)) (- k 1))))))))

(define (down-forward b p)
  (or (syntax-down (buffer-document b) (symbol->string (language-of b)) p #t) (error "No list after point")))

(define (down-backward b p)
  (or (syntax-down (buffer-document b) (symbol->string (language-of b)) p #f) (error "No list before point")))

;; The extent of N nodes from P: forward, or backward when N is negative.
(define (sexps b p n)
  (cond ((> n 0) (list p (sexp-end b p n)))
        ((< n 0) (list (sexp-start b p (- n)) p))
        (else (list p p))))

(define-command (kill-sexp s n)
  "Kill the next syntax node; with a count, that many.
A negative count kills the ones before point."
  (let* ((b (at-buffer s)) (extent (sexps b (point s) n)))
    (unless (= (car extent) (cadr extent))
      (delete-extents! s (list (list (car extent) (cadr extent) #f)) (if (< n 0) 'backward 'forward) 'new))))

(define-command (mark-sexp s n)
  "Mark the next syntax node: the region ends after it.
Again, the region takes in the node after it too. With a count, that
many; a negative count marks the ones before point."
  (let* ((b (at-buffer s)) (r (list-ref (ranges s) (view-primary (session-view s))))
         (anchor (if (sget s 'extend) (car r) (cadr r))))
    (sset! s 'extend #t)
    (view-set-ranges! (session-view s)
                      (list (list anchor (if (< n 0) (sexp-start b (cadr r) (- n)) (sexp-end b (cadr r) (max 1 n)))))
                      0)))

;;; Definitions

;; Definitions as a query's captures CS give them: (start end name).
(define (captured-definitions b cs)
  ;; Captures private to the query (@_head) are its own business.
  (let loop ((cs (filter (lambda (c) (not (string-prefix? "_" (car c)))) cs)) (acc '()))
    (cond ((null? cs) (reverse acc))
          ((string=? (car (car cs)) "definition")
           ;; Its name, if the next capture is one inside it.
           (let* ((c (car cs)) (next (and (pair? (cdr cs)) (cadr cs)))
                  (name (and next (string=? (car next) "name") (<= (cadr c) (cadr next)) (<= (caddr next) (caddr c)) next)))
             (loop (if name (cddr cs) (cdr cs))
                   (cons (list (cadr c) (caddr c)
                               (and name (document-substring (buffer-document b) (cadr name) (caddr name))))
                         acc))))
          (else (loop (cdr cs) acc)))))

(define (definitions-query b)
  (or (language-definitions (find-language (language-of b))) (error "This language says nothing of its definitions")))

;; The definitions of buffer B touching FROM to TO: (start end name), in
;; order. Those around the range are among them.
(define (definitions b from to)
  (captured-definitions b (syntax-captures b (definitions-query b) #:from from #:to to)))

;; How far a definition motion looks from point: past it, there is "no
;; definition" there, as a keystroke may not query a whole big file.
(define definition-reach (* 256 1024))

;; The definitions near P that SATISFY, looked for in a window around P
;; growing until one is found, it reaches the text's end, or
;; `definition-reach`: a motion costs what is near point.
(define (definitions-near b p satisfy #:backward [backward #f])
  (let ((len (document-length (buffer-document b))))
    (let grow ((size 16384))
      (let* ((from (if backward (max 0 (- p size)) (max 0 (- p 1))))
             (to (if backward (min len (+ p 1)) (min len (+ p size))))
             (found (filter satisfy (definitions b from to))))
        ;; Done when the window reaches the end it grows toward.
        (if (or (pair? found) (if backward (= from 0) (= to len)) (>= size definition-reach))
            found
            (grow (* size 4)))))))

(define-command (beginning-of-defun s n)
  "Move to the start of this definition, or the one before."
  (let ((b (at-buffer s)))
    (move! s (lambda (p)
               (let loop ((p p) (k (max 1 n)))
                 (if (= k 0) p
                     (let ((before (definitions-near b p (lambda (d) (< (car d) p)) #:backward #t)))
                       (if (null? before) (error "No definition before point")
                           (loop (apply max (map car before)) (- k 1))))))))))

(define-command (end-of-defun s n)
  "Move to the end of this definition, or the next one."
  (let ((b (at-buffer s)))
    (move! s (lambda (p)
               (let loop ((p p) (k (max 1 n)))
                 (if (= k 0) p
                     (let ((after (definitions-near b p (lambda (d) (> (cadr d) p)))))
                       (if (null? after) (error "No definition after point")
                           (loop (apply min (map cadr after)) (- k 1))))))))))

(define-command (mark-defun s n)
  "Mark the definition around point."
  (let* ((b (at-buffer s)) (p (point s)) (len (document-length (buffer-document b)))
         (around (filter (lambda (d) (and (<= (car d) p) (< p (cadr d))))
                         (definitions b (max 0 (- p 1)) (min len (+ p 1))))))
    (when (null? around) (error "No definition here"))
    ;; The innermost: the last to start.
    (let ((d (fold (lambda (d best) (if (> (car d) (car best)) d best)) (car around) around)))
      (sset! s 'extend #t)
      (view-set-ranges! (session-view s) (list (list (car d) (cadr d))) 0))))

;; imenu's asking, per session: its query goes over the whole text, on
;; a thread of its own, and the list opens when it is back.
(define (imenu-slot s)
  (or (sget s 'imenu-slot) (let ((slot (make-request-slot))) (sset! s 'imenu-slot slot) slot)))

(define-command (imenu s n)
  "Go to a definition of this buffer, choosing it by name.
The definitions are found off the editor's thread: in a big file the
list opens when they are."
  (let* ((b (at-buffer s)) (d (buffer-document b)) (query (definitions-query b)))
    (message! s "Finding definitions…")
    (request! (imenu-slot s)
              (lambda ()
                (filter caddr (captured-definitions b (syntax-query-later d (symbol->string (language-of b)) query 0 (document-length d)))))
              (lambda (named)
                (message! s #f)
                (if (null? named)
                    (message! s "No definitions here")
                    (completing-read s "Go to definition: "
                                     (map (lambda (e)
                                            (candidate (caddr e) #:annotation (number->string (line-number d (car e)))
                                                       #:target (target 'location (location d (car e)))))
                                          named)
                                     #:preview (lambda (s c) (act-default! s (candidate-target c)))
                                     #:history 'imenu)))
              #:fail (lambda (e) (message! s (error-text e))))))

;;; Selecting by nodes

(define-command (expand-region s n)
  "Select the syntax node at point, or the one around the selection."
  (let* ((b (at-buffer s)) (r (list-ref (ranges s) (view-primary (session-view s))))
         (from (min (car r) (cadr r))) (to (max (car r) (cadr r)))
         (node (let loop ((node (node-at b from #:to to)))
                 (cond ((not node) #f)
                       ;; One wider than what is selected.
                       ((and (sget s 'extend) (<= (node-start node) from) (>= (node-end node) to)
                             (or (< (node-start node) from) (> (node-end node) to)))
                        node)
                       ((and (not (sget s 'extend)) (< (node-start node) (node-end node))) node)
                       (else (loop (node-parent b node)))))))
    (unless node (error "Nothing wider to select"))
    (sset! s 'extend #t)
    (view-set-ranges! (session-view s) (list (list (node-start node) (node-end node))) 0)))

(define-command (describe-syntax-at-point s n)
  "Show the syntax nodes around point, the innermost last."
  (let ((b (at-buffer s)))
    (message! s (let loop ((node (node-at b (point s))) (path '()))
                  (if node
                      (loop (node-parent b node) (cons (node-kind node) path))
                      (string-join path " > "))))))

;;; The faces of capture names the faces' own names do not cover: the
;;; grammars' queries name a few things their own way.

(define-face string.special.key "A key written as a string, as JSON's are." #:inherit 'property)
(define-face text "Prose, as older queries name markup's text." #:inherit 'markup)
(define-face text.title "A heading, as older queries name it." #:inherit 'markup.heading)
(define-face text.strong "Strong text, as older queries name it." #:inherit 'markup.strong)
(define-face text.emphasis "Emphasised text, as older queries name it." #:inherit 'markup.emphasis)
(define-face text.literal "Literal text, as older queries name it." #:inherit 'markup.raw)
(define-face text.uri "A link's address, as older queries name it." #:inherit 'markup.link)
(define-face text.reference "A link's text, as older queries name it." #:inherit 'markup.link)
(define-face none "Text inside a node that is drawn in the text's own colours." #:inherit 'default #:bg #f)
(define-face embedded "Code inside a string (a template's substitution), in the text's own colours." #:inherit 'none)

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
