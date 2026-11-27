(import (techne test))
(test-file #:environment 'host)
(require "../../cloud/kv.scm")

(define (open-store root clock)
  (make-kv-store (string-append root "/kv/store") #:clock clock #:max-bytes 200))

(define (with-store body)
  (call-with-temporary-directory
    (lambda (root) (body root (open-store root (lambda () 1000))))))

(define (fails? thunk) (guard (e (#t #t)) (thunk) #f))
(define data (bytevector 0 1 2 255 195 169))
(define bytes string->utf8)

(define-test "reads, overwrites, and lists by namespace and prefix"
  (with-store
    (lambda (_ store)
      (check "nothing kept yet" #f (kv-store-get store "cache" "a"))
      (check "nor listed" '() (kv-store-list store "cache"))
      (check "a default" 'none (kv-store-get store "cache" "a" 'none))
      (kv-store-put! store "cache" "b" data)
      (kv-store-put! store "cache" "a" (bytes "one"))
      (kv-store-put! store "cache" "a" (bytes "two"))
      (check "the last write wins" (bytes "two") (kv-store-get store "cache" "a"))
      (check "bytes, as they were put" data (kv-store-get store "cache" "b"))
      (check "keys in order" '("a" "b") (kv-store-list store "cache"))
      (check "by prefix" '("b") (kv-store-list store "cache" #:prefix "b"))
      (check "at most the limit" '("a") (kv-store-list store "cache" #:limit 1))
      (check "a namespace of its own" #f (kv-store-get store "other" "a")))))

(define-test "distinguishes empty values, missing keys, and deletions"
  (with-store
    (lambda (_ store)
      (kv-store-put! store "cache" "f" (bytevector))
      (check "found, and empty" (dict 'found #t 'value (bytevector))
             (kv-store-handle store "cache" (dict 'op 'get 'key "f")))
      (check "not found" (dict 'found #f) (kv-store-handle store "cache" (dict 'op 'get 'key "g")))
      (kv-store-delete! store "cache" "f")
      (check "deleted" (dict 'found #f) (kv-store-handle store "cache" (dict 'op 'get 'key "f")))
      (kv-store-delete! store "cache" "f"))))

(define-test "expires entries at their deadline and excludes them from listings"
  (call-with-temporary-directory
    (lambda (root)
      (let* ((now 1000) (store (open-store root (lambda () now))))
        (kv-store-put! store "cache" "a" data)
        (kv-store-put! store "cache" "soon" (bytes "x") #:ttl 10)
        (set! now 1009)
        (check "kept until its time" (bytes "x") (kv-store-get store "cache" "soon"))
        (set! now 1010)
        (check "expired" #f (kv-store-get store "cache" "soon"))
        (check "and not listed" '("a") (kv-store-list store "cache"))))))

(define-test "reopens persistent values and expiry from disk"
  (call-with-temporary-directory
    (lambda (root)
      (let* ((now 1000) (clock (lambda () now)) (store (open-store root clock)))
        (kv-store-put! store "cache" "a" (bytes "two"))
        (kv-store-put! store "cache" "b" data)
        (kv-store-put! store "cache" "later" (bytes "y") #:ttl 5)
        (let ((again (open-store root clock)))
          (check "kept in its file" (bytes "two") (kv-store-get again "cache" "a"))
          (check "with its bytes" data (kv-store-get again "cache" "b"))
          (check "and its expiry" (bytes "y") (kv-store-get again "cache" "later"))
          (set! now 1005)
          (check "which holds there too" #f (kv-store-get again "cache" "later")))))))

(define-test "rejects invalid requests without changing stored values"
  (with-store
    (lambda (_ store)
      (kv-store-put! store "cache" "a" (bytes "two"))
      (check-true "an empty key" (fails? (lambda () (kv-store-put! store "cache" "" data))))
      (check-true "a key that is no string" (fails? (lambda () (kv-store-get store "cache" 'a))))
      (check-true "a key too long" (fails? (lambda () (kv-store-put! store "cache" (make-string 513 #\k) data))))
      (check-true "a value that is no bytes" (fails? (lambda () (kv-store-put! store "cache" "a" "text"))))
      (check-true "a namespace that names no file" (fails? (lambda () (kv-store-get store "../cache" "a"))))
      (check-true "nor an absolute one" (fails? (lambda () (kv-store-put! store "/tmp/x" "a" data))))
      (check-true "a TTL that is none" (fails? (lambda () (kv-store-put! store "cache" "a" data #:ttl -1))))
      (check-true "a limit past the most" (fails? (lambda () (kv-store-list store "cache" #:limit 1001))))
      (check-true "an operation unknown" (fails? (lambda () (kv-store-handle store "cache" (dict 'op 'drop)))))
      (check "the value is unchanged" (bytes "two") (kv-store-get store "cache" "a"))
      (check-true "namespace names"
        (and (kv-namespace-name? "cache-1_a") (not (kv-namespace-name? "Cache"))
             (not (kv-namespace-name? "")) (not (kv-namespace-name? "a/b")) (not (kv-namespace-name? "a.b")))))))

(define-test "enforces byte limits and reclaims replaced or expired values"
  (call-with-temporary-directory
    (lambda (root)
      (let* ((now 1000) (store (open-store root (lambda () now))))
        (kv-store-put! store "cache" "a" (bytes "two"))
        (check-true "a namespace full" (fails? (lambda () (kv-store-put! store "cache" "big" (make-bytevector 300 1)))))
        (check "the one refused is not kept" #f (kv-store-get store "cache" "big"))
        (check "and the rest is" (bytes "two") (kv-store-get store "cache" "a"))
        (kv-store-put! store "small" "k" (make-bytevector 150 1))
        (kv-store-put! store "small" "k" (make-bytevector 150 2))
        (check "replaced within the bound" (make-bytevector 150 2) (kv-store-get store "small" "k"))
        (check-true "bytes are counted" (fails? (lambda () (kv-store-put! store "wide" (make-string 100 #\é) (make-bytevector 1 0)))))
        (kv-store-put! store "brief" "a" (make-bytevector 150 1) #:ttl 5)
        (check-true "full while it lasts" (fails? (lambda () (kv-store-put! store "brief" "b" (make-bytevector 150 1)))))
        (set! now 1005)
        (kv-store-put! store "brief" "b" (make-bytevector 150 1))
        (check "room again once it expired" '("b") (kv-store-list store "brief"))))))

(define-test "refuses corrupt on-disk namespaces"
  (with-store
    (lambda (root store)
      (kv-store-put! store "other" "k" data)
      (replace-file (string-append root "/kv/store/broken.kv") "((\"k\" not-base64 #f))")
      (check-true "a file that is not one" (fails? (lambda () (kv-store-get store "broken" "k"))))
      (check-true "nor written over" (fails? (lambda () (kv-store-put! store "broken" "k" data)))))))

(define-test "dispatches put, list, and delete requests"
  (with-store
    (lambda (_ store)
      (check "put, as asked" #t (kv-store-handle store "ops" (dict 'op 'put 'key "k" 'value data 'ttl #f)))
      (check "list, as asked" '("k") (kv-store-handle store "ops" (dict 'op 'list 'prefix "" 'limit 10)))
      (check "delete, as asked" #t (kv-store-handle store "ops" (dict 'op 'delete 'key "k"))))))
