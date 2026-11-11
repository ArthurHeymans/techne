(require "../../test.scm")
(require "../../cloud/tokens.scm")
(import (techne crypto))

(define key (make-token-key))
(define (fails? thunk) (guard (e (#t #t)) (thunk) #f))
(define token (make-token key "arthur" #:projects '("weather" "notes") #:scopes '("eval" "deploy") #:expires 2000))
(define claims (verify-token key token #:now 1000))

(check "a token holds, with its claims" "arthur" (dict-ref claims "sub"))
(check "its projects" '("weather" "notes") (dict-ref claims "projects"))
(check-true "it allows what it was made for" (token-allows? claims "weather" "eval"))
(check "not another scope" #f (token-allows? claims "weather" "prod"))
(check "not another project" #f (token-allows? claims "mail" "eval"))
(check "not past its expiry" #f (verify-token key token #:now 2000))
(check "not under another key" #f (verify-token (make-token-key) token #:now 1000))
(check "not revoked" #f (verify-token key token #:now 1000 #:revoked? (lambda (id) (equal? id (dict-ref claims "id")))))

;; Changed: its claims, its MAC, or what is no token at all.
(define (with-last-char s ch) (string-append (substring s 0 (- (string-length s) 1)) (string ch)))
(check "not with its MAC changed" #f
       (verify-token key (with-last-char token (if (char=? (string-ref token (- (string-length token) 1)) #\A) #\B #\A)) #:now 1000))
(let* ((dot (let loop ((i (- (string-length token) 1))) (if (char=? (string-ref token i) #\.) i (loop (- i 1)))))
       (mac (substring token dot (string-length token)))
       (forged (string-append "tk1."
                              (base64url-encode "{\"id\":\"x\",\"sub\":\"mallory\",\"projects\":[\"weather\"],\"scopes\":[\"prod\"],\"exp\":9999,\"chain\":[]}")
                              mac)))
  (check "not with other claims under its MAC" #f (verify-token key forged #:now 1000)))
(for-each (lambda (junk) (check "what is no token" #f (verify-token key junk #:now 1000)))
          (list "" "tk1." "tk1.." "tk1.a.b" "tk2.a.b" 42 #f (make-string 5000 #\a)))
(check-true "a scope that is none" (fails? (lambda () (make-token key "a" #:projects '("p") #:scopes '("root") #:expires 2000))))

;; Made from another: no more than its maker, and revoked with it.
(define agent (attenuate-token key token "agent" #:projects '("weather") #:scopes '("eval") #:expires 1500 #:now 1000))
(define agent-claims (verify-token key agent #:now 1000))
(check "a token made from it holds" "agent" (dict-ref agent-claims "sub"))
(check "with its maker in its chain" (list (dict-ref claims "id")) (dict-ref agent-claims "chain"))
(check "it has what it was given" #t (token-allows? agent-claims "weather" "eval"))
(check "and no more" #f (token-allows? agent-claims "notes" "eval"))
(check "it expires sooner" #f (verify-token key agent #:now 1500))
(check "revoking its maker revokes it" #f
       (verify-token key agent #:now 1000 #:revoked? (lambda (id) (equal? id (dict-ref claims "id")))))
(check-true "more scopes than its maker" (fails? (lambda () (attenuate-token key token "agent" #:scopes '("eval" "prod") #:now 1000))))
(check-true "more projects" (fails? (lambda () (attenuate-token key token "agent" #:projects '("mail") #:now 1000))))
(check-true "a longer life" (fails? (lambda () (attenuate-token key token "agent" #:expires 2001 #:now 1000))))
(check-true "from one that holds no more" (fails? (lambda () (attenuate-token key token "agent" #:now 2000))))
(check-true "nor from one of its own, for more" (fails? (lambda () (attenuate-token key agent "sub" #:scopes '("deploy") #:now 1000))))
(let ((sub (verify-token key (attenuate-token key agent "sub" #:now 1000) #:now 1000)))
  (check "its maker's, by default" '("eval") (dict-ref sub "scopes"))
  (check "the chain grows" 2 (length (dict-ref sub "chain"))))

;; All of a tenant's projects, and no other tenant's.
(define wide (verify-token key (make-token key "acme" #:projects '("acme/*") #:scopes '("deploy") #:expires 2000) #:now 1000))
(check "a tenant's project" #t (token-allows? wide "acme/shop" "deploy"))
(check "not another tenant's" #f (token-allows? wide "acme-two/shop" "deploy"))
(check "nor one named to look like it" #f (token-allows? wide "bolt/acme/shop" "deploy"))
(check "nor the tenant alone" #f (token-allows? wide "acme" "deploy"))
(check "nor what is no name" #f (token-allows? wide #f "deploy"))
(let ((wide-token (make-token key "acme" #:projects '("acme/*") #:scopes '("deploy") #:expires 2000)))
  (check "one of them, for an agent" '("acme/shop")
         (dict-ref (verify-token key (attenuate-token key wide-token "agent" #:projects '("acme/shop") #:now 1000) #:now 1000) "projects"))
  (check-true "not another tenant's, for an agent"
              (fails? (lambda () (attenuate-token key wide-token "agent" #:projects '("bolt/shop") #:now 1000))))
  (check-true "nor all of another's" (fails? (lambda () (attenuate-token key wide-token "agent" #:projects '("bolt/*") #:now 1000)))))
(check-true "one project is not all of them"
            (fails? (lambda () (attenuate-token key (make-token key "a" #:projects '("acme/shop") #:scopes '("deploy") #:expires 2000)
                                                "agent" #:projects '("acme/*") #:now 1000))))

(test-summary)
