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

(provide read-bindings manifest-file secrets-file)

(define manifest-file "manifest.scm"
  "The name of a deployment's manifest, in its directory.")

(define secrets-file ".secrets.scm"
  "The name of the file of a deployment's secrets, in its directory.")

;; What FILE holds, read as data, or DEFAULT if it is not there.
(define (read-data file default)
  (if (file-exists? file)
      (let ((data (call-with-input-file file read)))
        (if (eof-object? data) default data))
      default))

(define (read-bindings directory)
  "Return the bindings of the deployment in DIRECTORY, from its manifest.
A dict of `env`, what its requests are given, by the bindings' names,
and `stores`, the namespace of each KV binding by the binding's name, a
string. A deployment without a manifest has none. A manifest that is
not one, a binding of no known kind, or a secret that is not among the
secrets is an error that says which."
  (let* ((manifest (read-data (string-append directory "/" manifest-file) (dict)))
         (secrets (read-data (string-append directory "/" secrets-file) (dict)))
         (bindings (if (dict? manifest) (dict-ref manifest 'bindings (dict)) (error "The manifest is not a dict:" manifest))))
    (unless (dict? bindings) (error "The manifest's bindings are not a dict:" bindings))
    (unless (dict? secrets) (error "The secrets are not a dict:" secrets))
    (fold (lambda (name result)
            (let ((what (dict-ref bindings name))
                  (bound (lambda (value) (dict-set result 'env (dict-set (dict-ref result 'env) name value)))))
              (unless (symbol? name) (error "A binding's name is a symbol:" name))
              (unless (and (list? what) (= (length what) 2) (string? (cadr what)))
                (error "A binding is (kv NAMESPACE), (secret NAME) or (text STRING):" name what))
              (case (car what)
                ((kv)
                 (unless (kv-namespace-name? (cadr what)) (error "Not a namespace name:" (cadr what)))
                 (dict-set (bound (dict 'kind 'kv 'name (symbol->string name)))
                           'stores (dict-set (dict-ref result 'stores) (symbol->string name) (cadr what))))
                ((secret)
                 (let ((value (dict-ref secrets (cadr what) #f)))
                   (unless (string? value) (error "No such secret, for the binding:" (cadr what) name))
                   (bound value)))
                ((text) (bound (cadr what)))
                (else (error "A binding is (kv NAMESPACE), (secret NAME) or (text STRING):" name what)))))
          (dict 'env (dict) 'stores (dict))
          (dict-keys bindings))))
