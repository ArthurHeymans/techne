;;; Which project a request is for, by its host name
;;; (doc/serverless.md, "Architecture on one VPS"): a front serving many
;;; tenants maps `project.tenant.DOMAIN` to the target `tenant/project`
;;; that a fleet is asked for (crates/techne-host/src/fleet.rs).
;;;
;;;   (http-serve listener (host-routing "techne.example" host-fetch))
;;;
;;; A host that is none of the domain's, or names no tenant and project,
;;; is answered 404 here, before anything is asked of anyone. Names are
;;; lowercase letters, digits and `-`, as a host's labels are; a host's
;;; case does not matter, nor its port.

(require "../http/http.scm")

(provide host-target host-routing cloud-name?)

(define (cloud-name? name)
  "Return #t if NAME can name a tenant or a project.
A string of 1 to 40 lowercase letters, digits and `-`, with no `-` at
either end."
  (and (string? name)
       (<= 1 (string-length name) 40)
       (not (char=? (string-ref name 0) #\-))
       (not (char=? (string-ref name (- (string-length name) 1)) #\-))
       (let loop ((i 0))
         (or (= i (string-length name))
             (let ((ch (string-ref name i)))
               (and (or (char<=? #\a ch #\z) (char<=? #\0 ch #\9) (char=? ch #\-))
                    (loop (+ i 1))))))))

;; S up to its first CH, and after it; S and #f without one.
(define (split-first s ch)
  (let loop ((i 0))
    (cond ((= i (string-length s)) (cons s #f))
          ((char=? (string-ref s i) ch) (cons (substring s 0 i) (substring s (+ i 1) (string-length s))))
          (else (loop (+ i 1))))))

(define (host-target host domain)
  "Return the target HOST names under DOMAIN, `tenant/project`, or #f.
HOST is a request's host header, `project.tenant.DOMAIN`, with a port or
without, in any case; #f for a host of another domain, with more or
fewer labels, or whose labels name no tenant and project."
  (and (string? host)
       (let* ((host (string-downcase (car (split-first host #\:))))
              (suffix (string-append "." (string-downcase domain)))
              (n (- (string-length host) (string-length suffix))))
         (and (> n 0)
              (string=? (substring host n (string-length host)) suffix)
              (let* ((labels (split-first (substring host 0 n) #\.))
                     (project (car labels))
                     (tenant (cdr labels)))
                (and tenant (cloud-name? project) (cloud-name? tenant)
                     (string-append tenant "/" project)))))))

(define (host-routing domain serve)
  "Return a handler asking SERVE for each request's target under DOMAIN.
(SERVE target request) answers REQUEST for TARGET, `tenant/project`, as
its host header names it (`host-target`); a request whose host names
none is answered 404."
  (lambda (request)
    (let ((target (host-target (request-header request "host") domain)))
      (if target
          (serve target request)
          (response "Not Found" #:status 404)))))
