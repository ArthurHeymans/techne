(require "../../test.scm")
(require "../../cloud/hosts.scm")

(check "a project of a tenant" "acme/shop" (host-target "shop.acme.techne.example" "techne.example"))
(check "whatever its case, and its port" "acme/shop" (host-target "Shop.ACME.Techne.Example:8443" "techne.example"))
(check "digits and dashes" "a-1/b-2" (host-target "b-2.a-1.techne.example" "techne.example"))
(for-each (lambda (host)
            (check (string-append "no target: " (if (string? host) host "none")) #f (host-target host "techne.example")))
          (list "techne.example" "acme.techne.example" "x.shop.acme.techne.example" "shop.acme.other.example"
                "shop.acme.techne.example.evil.test" "shop.acmetechne.example" ".acme.techne.example" "shop..techne.example"
                "-shop.acme.techne.example" "shop.acme-.techne.example" "sh_op.acme.techne.example" "shop/x.acme.techne.example"
                (string-append (make-string 41 #\a) ".acme.techne.example") "" #f))
(check-true "names" (and (cloud-name? "a") (cloud-name? "a-b-1") (not (cloud-name? "")) (not (cloud-name? "A"))
                         (not (cloud-name? "a/b")) (not (cloud-name? "a.b")) (not (cloud-name? "..")) (not (cloud-name? 'a))))

(define asked '())
(define route (host-routing "techne.example" (lambda (target request) (set! asked (cons target asked)) (dict 'status 200 'body target))))
(define (request host) (dict 'method "GET" 'path "/" 'headers (if host (dict "host" host) (dict)) 'body (bytevector)))
(check "served for its target" "acme/shop" (dict-ref (route (request "shop.acme.techne.example")) 'body))
(check "another host: not found" 404 (dict-ref (route (request "shop.acme.elsewhere.example")) 'status))
(check "no host: not found" 404 (dict-ref (route (request #f)) 'status))
(check "and no one asked" '("acme/shop") asked)

(test-summary)
