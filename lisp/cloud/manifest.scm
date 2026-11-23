;;; A deployment's manifest: the bindings its requests are given
;;; (doc/serverless.md, "Concepts"), read from `manifest.scm` beside its
;;; entry module, as data, never evaluated:
;;;
;;;   {bindings {cache    (kv "weather-cache")
;;;              api-key  (secret "WEATHER_API_KEY")
;;;              greeting (text "hello")}}
;;;
;;; Each binding has the name the deployment knows it by, a symbol, and
;;; what it stands for: a KV namespace by its name, a secret by its name
;;; among the secrets, or a text given here. Secrets are kept apart, in
;;; `.secrets.scm`, a dict of their names and values, strings, which is
;;; no part of the deployment's files:
;;;
;;;   {"WEATHER_API_KEY" "..."}
;;;
;;; `read-bindings` returns what a host needs of them: the `env` the
;;; deployment's requests are given, in which a secret or a text is its
;;; string and a KV binding the handle `kv-get` takes, which names the
;;; binding and not the namespace; and the namespace each KV binding
;;; stands for, which only the host knows.

(require "kv.scm")

(provide read-bindings validate-bindings manifest-file secrets-file)

(define manifest-file "manifest.scm"
  "The name of a deployment's manifest, in its directory.")

(define secrets-file ".secrets.scm"
  "The name of the file of a deployment's secrets, in its directory.")

;; What FILE holds, read as data, or DEFAULT if it is not there.
;; Rejects trailing datums after the first expression.
(define (read-data file default)
  (if (file-exists? file)
      (call-with-input-file file
        (lambda (port)
          (let ((data (read port)))
            (if (eof-object? data)
                default
                (let ((extra (read port)))
                  (unless (eof-object? extra)
                    (error "Unexpected trailing datum in:" file))
                  data)))))
      default))

(define (valid-binding-kind? kind)
  (and (symbol? kind) (memq kind '(kv secret text)) #t))

(define (check-binding-entry name what)
  (unless (symbol? name) (error "A binding's name is a symbol:" name))
  (unless (and (list? what) (= (length what) 2) (valid-binding-kind? (car what)) (string? (cadr what)))
    (error "A binding is (kv NAMESPACE), (secret NAME) or (text STRING):" name what))
  (car what))

(define (validate-bindings bindings declared)
  "Validate BINDINGS from `manifest.scm` against DECLARED.
BINDINGS is the `bindings` dict of `manifest.scm`, and DECLARED is a
list of `(NAME KIND)` pairs from a package's `(cloud ...)` clause,
where NAME is a symbol and KIND is `kv`, `secret`, or `text`.
Every declared binding must be present in BINDINGS with the exact
matching kind, and BINDINGS may not contain extra undeclared bindings.
Signals an error naming the binding, expected kind, and actual kind on
any mismatch."
  (unless (dict? bindings) (error "The manifest's bindings are not a dict:" bindings))
  (unless (list? declared) (error "Declared bindings are a list of (NAME KIND):" declared))
  (let ((expected (fold (lambda (entry table)
                          (unless (and (list? entry) (= (length entry) 2)
                                       (symbol? (car entry)) (valid-binding-kind? (cadr entry)))
                            (error "A declared binding is (NAME kv|secret|text):" entry))
                          (when (dict-ref table (car entry) #f)
                            (error "Binding declared twice:" (car entry)))
                          (dict-set table (car entry) (cadr entry)))
                        (dict) declared)))
    (for-each (lambda (name)
                (let* ((what (dict-ref bindings name))
                       (actual (check-binding-entry name what))
                       (want (dict-ref expected name #f)))
                  (unless want
                    (error (string-append "Undeclared binding " (symbol->string name)
                                          ": expected none, got " (symbol->string actual))))
                  (unless (eq? actual want)
                    (error (string-append "Binding " (symbol->string name)
                                          " has kind " (symbol->string actual)
                                          ", expected " (symbol->string want))))))
              (dict-keys bindings))
    (for-each (lambda (entry)
                (let ((name (car entry))
                      (want (cadr entry)))
                  (unless (dict-ref bindings name #f)
                    (error (string-append "Missing binding " (symbol->string name)
                                          ": expected " (symbol->string want) ", got none")))))
              declared)
    #t))

(define (read-bindings directory [declared #f])
  "Return the bindings of the deployment in DIRECTORY, from its manifest.
A dict of `env`, what its requests are given, by the bindings' names,
and `stores`, the namespace of each KV binding by the binding's name, a
string. When DECLARED is given (not `#f`), it is a list of `(NAME KIND)`
pairs from a package's `(cloud ...)` clause and `manifest.scm` is
validated against it via `validate-bindings` before resolving secrets.
A deployment without a manifest has none (which fails if DECLARED is
non-empty). A manifest that is not one, a binding of no known kind, a
contract mismatch, or a secret that is not among the secrets is an error
that says which."
  (let* ((manifest (read-data (string-append directory "/" manifest-file) (dict)))
         (secrets (read-data (string-append directory "/" secrets-file) (dict)))
         (bindings (if (dict? manifest) (dict-ref manifest 'bindings (dict)) (error "The manifest is not a dict:" manifest))))
    (unless (dict? bindings) (error "The manifest's bindings are not a dict:" bindings))
    (unless (dict? secrets) (error "The secrets are not a dict:" secrets))
    (when declared
      (validate-bindings bindings declared))
    (fold (lambda (name result)
            (let ((what (dict-ref bindings name))
                  (bound (lambda (value) (dict-set result 'env (dict-set (dict-ref result 'env) name value)))))
              (case (check-binding-entry name what)
                ((kv)
                 (unless (kv-namespace-name? (cadr what)) (error "Not a namespace name:" (cadr what)))
                 (dict-set (bound (dict 'kind 'kv 'name (symbol->string name)))
                           'stores (dict-set (dict-ref result 'stores) (symbol->string name) (cadr what))))
                ((secret)
                 (let ((value (dict-ref secrets (cadr what) #f)))
                   (unless (string? value) (error "No such secret, for the binding:" (cadr what) name))
                   (bound value)))
                ((text) (bound (cadr what))))))
          (dict 'env (dict) 'stores (dict))
          (dict-keys bindings))))
