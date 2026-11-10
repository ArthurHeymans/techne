(require "../../test.scm")
(require "../../cloud/manifest.scm")

(define root (string-append "/tmp/techne-manifest-tests-" (number->string (exact (floor (* 1000 (current-second)))))))
(create-directory root #o700)

(define (fails-with thunk)
  (guard (e (#t (condition/report-string e))) (thunk) #f))

(check "no manifest, no bindings" (dict 'env (dict) 'stores (dict)) (read-bindings root))

(replace-file (string-append root "/manifest.scm")
              "{bindings {cache (kv \"weather-cache\") api-key (secret \"API_KEY\") greeting (text \"hello\")}}")
(check-true "a secret that is not there" (string-contains (fails-with (lambda () (read-bindings root))) "API_KEY"))

(replace-file (string-append root "/.secrets.scm") "{\"API_KEY\" \"s3cret\" \"OTHER\" \"unused\"}")
(let ((bindings (read-bindings root)))
  (check "what requests are given"
         (dict 'cache (dict 'kind 'kv 'name "cache") 'api-key "s3cret" 'greeting "hello")
         (dict-ref bindings 'env))
  (check "the namespaces, which they are not" (dict "cache" "weather-cache") (dict-ref bindings 'stores)))

(define (refused manifest)
  (replace-file (string-append root "/manifest.scm") manifest)
  (and (fails-with (lambda () (read-bindings root))) #t))

(check-true "a binding of no kind" (refused "{bindings {a (queue \"q\")}}"))
(check-true "a namespace that names no file" (refused "{bindings {a (kv \"../x\")}}"))
(check-true "a binding that is no list" (refused "{bindings {a \"x\"}}"))
(check-true "a name that is no symbol" (refused "{bindings {\"a\" (text \"x\")}}"))
(check-true "bindings that are no dict" (refused "{bindings (a b)}"))
(check-true "a manifest that is no dict" (refused "(bindings)"))
;; Read, not evaluated.
(check-true "a manifest is data" (refused "(begin (delete-path \"/\" #t) {})"))

(delete-path root #t)
(test-summary)
