;;; Tokens: what a client shows to act on a project (doc/serverless.md,
;;; "Authority"). A token is its claims and a MAC of them under a key
;;; only the service has, so the service keeps no list of tokens to know
;;; one is its own:
;;;
;;;   tk1.<claims, JSON, in URL-safe base64>.<HMAC-SHA-256 of that, likewise>
;;;
;;; Its claims are a dict: `id`, its own name; `sub`, whom it is for;
;;; `projects`, the projects it may act on; `scopes`, what it may do
;;; there (`inspect`, `eval`, `prod`, `deploy`); `exp`, the second it
;;; expires at; and `chain`, the ids of the tokens it was made from, the
;;; first first.
;;;
;;; A token made from another (`attenuate-token`, for an agent) has at
;;; most its maker's projects, scopes and expiry, never more, and its
;;; maker's id in its chain: revoking a token, by its id, revokes those
;;; made from it. Who is revoked is the caller's to keep (REVOKED?).
;;;
;;; Nothing here is secret but the key. A token is checked in a time
;;; that does not depend on how much of its MAC is right.

(import (techne crypto))

(provide token-scopes make-token-key make-token verify-token token-allows? attenuate-token)

(define token-scopes '("inspect" "eval" "prod" "deploy")
  "The scopes a token may have, as strings.")

(define prefix "tk1.")

(define (make-token-key)
  "Return a new key to make and verify tokens with, a bytevector.
The world must be granted entropy."
  (random-bytes 32))

(define (subset? items of)
  (every (lambda (item) (and (member item of) #t)) items))

;; Whether PROJECTS, a token's, have PROJECT: by its name, or, for
;; `tenant/project`, by `tenant/*`, all of a tenant's.
(define (covers? projects project)
  (or (and (member project projects) #t)
      (any (lambda (p)
             (let ((n (string-length p)))
               (and (>= n 2)
                    (string=? (substring p (- n 2) n) "/*")
                    (not (string-suffix? "/*" project))
                    (string-prefix? (substring p 0 (- n 1)) project))))
           projects)))

(define (strings? x)
  (and (list? x) (every string? x)))

;; CLAIMS if they are a token's, else an error saying what is not.
(define (check-claims claims)
  (unless (and (dict? claims)
               (string? (dict-ref claims "id" #f))
               (string? (dict-ref claims "sub" #f))
               (strings? (dict-ref claims "projects" #f))
               (strings? (dict-ref claims "scopes" #f))
               (subset? (dict-ref claims "scopes") token-scopes)
               (real? (dict-ref claims "exp" #f))
               (strings? (dict-ref claims "chain" #f)))
    (error "Not a token's claims:" claims))
  claims)

;; JSON's arrays are read as vectors: CLAIMS, as read, with lists.
(define (listed claims)
  (if (dict? claims)
      (fold (lambda (name claims)
              (let ((value (dict-ref claims name #f)))
                (if (vector? value) (dict-set claims name (vector->list value)) claims)))
            claims '("projects" "scopes" "chain"))
      claims))

(define (seal key claims)
  (let ((payload (base64url-encode (json->string claims))))
    (string-append prefix payload "." (base64url-encode (hmac-sha-256 key payload)))))

(define (make-token key subject #:projects projects #:scopes scopes #:expires expires)
  "Return a token for SUBJECT, a string, made with KEY.
It may act on PROJECTS, a list of strings, within SCOPES, a list of
`token-scopes`, until EXPIRES, a time in seconds. The world must be
granted entropy, for its id."
  (seal key (check-claims (dict "id" (hex-encode (random-bytes 8))
                                "sub" subject
                                "projects" projects
                                "scopes" scopes
                                "exp" expires
                                "chain" '()))))

;; The index of the last CH in S, or #f.
(define (last-index s ch)
  (let loop ((i (- (string-length s) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref s i) ch) i)
          (else (loop (- i 1))))))

(define (verify-token key token #:now [now (current-second)] #:revoked? [revoked? (lambda (id) #f)])
  "Return the claims of TOKEN if it was made with KEY and still holds.
#f otherwise: for what is no token, one of another key, one changed, one
past its expiry at NOW, or one REVOKED? says is revoked, by its id or by
the id of a token it was made from."
  (and (string? token)
       (<= (string-length token) 4096)
       (string-prefix? prefix token)
       (let* ((body (substring token (string-length prefix) (string-length token)))
              (dot (last-index body #\.)))
         (and dot
              (let ((payload (substring body 0 dot))
                    (mac (guard (e (#t #f)) (base64url-decode (substring body (+ dot 1) (string-length body))))))
                (and mac
                     (digest=? mac (hmac-sha-256 key payload))
                     ;; Its own from here on: made by `seal`.
                     (let ((claims (guard (e (#t #f))
                                     (check-claims (listed (string->json (utf8->string (base64url-decode payload))))))))
                       (and claims
                            (< now (dict-ref claims "exp"))
                            (not (revoked? (dict-ref claims "id")))
                            (not (any revoked? (dict-ref claims "chain")))
                            claims))))))))

(define (token-allows? claims project scope)
  "Return #t if CLAIMS, a token's, allow SCOPE on PROJECT.
A token has a project by its name, or, a tenant's (`tenant/project`),
by `tenant/*`, which is all of that tenant's."
  (and (string? project)
       (covers? (dict-ref claims "projects") project)
       (member scope (dict-ref claims "scopes"))
       #t))

(define (attenuate-token key token subject #:projects [projects #f] #:scopes [scopes #f] #:expires [expires #f]
                         #:now [now (current-second)] #:revoked? [revoked? (lambda (id) #f)])
  "Return a token for SUBJECT made from TOKEN, which must hold.
TOKEN is checked with KEY, at NOW and with REVOKED?, as `verify-token`
does. The new one has TOKEN's projects, scopes and expiry, or PROJECTS,
SCOPES and EXPIRES if given, which must be no more than TOKEN's: more is
an error. Revoking TOKEN revokes it."
  (let ((maker (or (verify-token key token #:now now #:revoked? revoked?) (error "The token does not hold"))))
    (let ((projects (or projects (dict-ref maker "projects")))
          (scopes (or scopes (dict-ref maker "scopes")))
          (expires (or expires (dict-ref maker "exp"))))
      (unless (and (strings? projects) (every (lambda (p) (covers? (dict-ref maker "projects") p)) projects))
        (error "A token is made for its maker's projects at most:" projects))
      (unless (and (strings? scopes) (subset? scopes (dict-ref maker "scopes")))
        (error "A token is made with its maker's scopes at most:" scopes))
      (unless (and (real? expires) (<= expires (dict-ref maker "exp")))
        (error "A token is made to last as long as its maker at most:" expires))
      (seal key (check-claims (dict "id" (hex-encode (random-bytes 8))
                                    "sub" subject
                                    "projects" projects
                                    "scopes" scopes
                                    "exp" expires
                                    "chain" (append (dict-ref maker "chain") (list (dict-ref maker "id")))))))))
