(require "../../test.scm")
(require "../../http/http.scm")
(require "../../http/debug-route.scm")

(define credential (make-debug-credential))
(define seen '())

;; Answers where it was served, and remembers the headers it was given.
(define (serve request debug)
  (set! seen (cons (dict-ref request 'headers) seen))
  (if (equal? (dict-ref request 'path) "/string")
      (if debug "debug" "production")
      (response (if debug "debug" "production") #:headers (dict "Cache-Control" "public, max-age=60"))))

(define route (debug-routing serve credential))

(define (request path . headers)
  (dict 'method "GET" 'path path 'headers (apply dict headers) 'body (bytevector)))

(check "a credential is long" 32 (string-length credential))
(check-true "and new each time" (not (equal? credential (make-debug-credential))))

;; Without the credential: served as any other, unmarked.
(let ((r (route (request "/"))))
  (check "no credential: production" "production" (dict-ref r 'body))
  (check "its headers as given" "public, max-age=60" (dict-ref (dict-ref r 'headers) "Cache-Control"))
  (check "unmarked" #f (dict-ref (dict-ref r 'headers) "techne-debug" #f)))

;; Another credential routes nothing, and the deployment does not see it.
(set! seen '())
(let ((r (route (request "/" "techne-debug-route" (make-debug-credential) "accept" "*/*"))))
  (check "another credential: production" "production" (dict-ref r 'body))
  (check "the header is taken from the request" (dict "accept" "*/*") (car seen)))
(check "an empty credential: production" "production" (dict-ref (route (request "/" "techne-debug-route" "")) 'body))

;; The credential routes to the debug instance; its response is marked.
(set! seen '())
(let* ((r (route (request "/" "techne-debug-route" credential)))
       (headers (dict-ref r 'headers)))
  (check "the credential: debug" "debug" (dict-ref r 'body))
  (check "the header is taken from it too" (dict) (car seen))
  (check "kept by no shared cache" "private, no-store" (dict-ref headers "cache-control"))
  (check "whatever the deployment said" #f (dict-ref headers "Cache-Control" #f))
  (check "marked with a number of its own" 16 (string-length (dict-ref headers "techne-debug")))
  (check-true "which is not the credential" (not (equal? credential (dict-ref headers "techne-debug")))))

(let ((r (route (request "/string" "techne-debug-route" credential))))
  (check "a string response: debug" "debug" (dict-ref r 'body))
  (check "marked too" "private, no-store" (dict-ref (dict-ref r 'headers) "cache-control")))

(test-summary)
