;;; Routing requests to a deployment's debug instance (doc/serverless.md,
;;; "Debugging in production"), for a front that hands its requests to a
;;; host: which requests go there is decided here, in Lisp, and the host
;;; only serves them where it is told.
;;;
;;;   (http-serve listener
;;;               (debug-routing (lambda (request debug)
;;;                                (host-fetch "app" request #:debug debug))
;;;                              credential))
;;;
;;; A request goes to the debug instance if its `techne-debug-route` header
;;; is the credential, which whoever holds the debugging session was given
;;; and no one else can guess. Nothing else is routed there. The header is
;;; taken from every request before it is served, so that no deployment
;;; reads a credential, and a response from the debug instance says so
;;; (`techne-debug`, a number of its own, not the credential) and is kept
;;; by no shared cache.

(require "http.scm")
(import (techne crypto))

(provide debug-route-header make-debug-credential debug-routing)

(define debug-route-header "techne-debug-route"
  "The name of the request header that carries a routing credential.")

(define (make-debug-credential)
  "Return a new routing credential, a string no one can guess.
The world must be granted entropy."
  (hex-encode (random-bytes 16)))

;; RESPONSE, a debug instance's, marked as one: no shared cache keeps it.
;; What is no response is left for the server to refuse.
(define (mark-debugged response)
  (let ((response (if (string? response) (dict 'body response) response)))
    (if (dict? response)
        (let* ((given (dict-ref response 'headers (dict)))
               (headers (fold (lambda (name h)
                                (if (member (string-downcase name) '("cache-control" "techne-debug")) (dict-delete h name) h))
                              given (dict-keys given))))
          (dict-set response 'headers
                    (dict-set (dict-set headers "cache-control" "private, no-store")
                              "techne-debug" (hex-encode (random-bytes 8)))))
        response)))

;; RESPONSE, not a debug instance's, saying that what is answered
;; depends on the routing header: a cache that kept it answers no
;; request that shows a credential with it.
(define (vary-on-route response)
  (let ((response (if (string? response) (dict 'body response) response)))
    (if (dict? response)
        (let* ((given (dict-ref response 'headers (dict)))
               (names (filter (lambda (name) (string=? (string-downcase name) "vary")) (dict-keys given)))
               (said (append-map (lambda (name)
                                   (let ((value (dict-ref given name)))
                                     (if (string? value) (list value) value)))
                                 names))
               (headers (fold (lambda (name h) (dict-delete h name)) given names)))
          (dict-set response 'headers
                    (dict-set headers "vary"
                              (fold (lambda (value all) (string-append value ", " all)) debug-route-header (reverse said)))))
        response)))

(define (debug-routing serve credential)
  "Return a handler routing requests with CREDENTIAL to a debug instance.
(SERVE request debug) answers REQUEST, from the debug instance if DEBUG.
A request whose `techne-debug-route` header is CREDENTIAL, a string, is
served with DEBUG and its response marked: a `techne-debug` header, and
a cache-control that lets no shared cache keep it. Any other is served
without, and says that it varies with the header, so that a cache
answers no request showing a credential with it. SERVE never sees the
header. CREDENTIAL may be a procedure
instead, called for each request that shows one: the credentials that
route now, a list of strings, each compared, none if it is empty."
  (lambda (request)
    (let* ((headers (dict-ref request 'headers (dict)))
           (given (dict-ref headers debug-route-header #f))
           (request (if given (dict-set request 'headers (dict-delete headers debug-route-header)) request)))
      (if (and given
               (if (procedure? credential)
                   ;; Each compared, whichever is the one.
                   (fold (lambda (c found) (or (digest=? given c) found)) #f (credential))
                   (digest=? given credential)))
          (mark-debugged (serve request #t))
          (vary-on-route (serve request #f))))))
